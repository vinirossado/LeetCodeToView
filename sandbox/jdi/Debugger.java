import com.sun.jdi.*;
import com.sun.jdi.connect.*;
import com.sun.jdi.event.*;
import com.sun.jdi.request.*;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.PrintStream;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

// Driver JDI do spike (Fase 0.5): lança a classe alvo com com.sun.jdi.LaunchingConnector
// (fica no mesmo processo/jail que o alvo — não há separação debugger/debuggee ainda,
// isso é uma simplificação deliberada pra validar captura de eventos primeiro).
// Emite 1 linha JSON por evento de step no stdout.
public class Debugger {

    static final int MAX_DEPTH = 3;
    static final int MAX_ARRAY_ELEMENTS = 20;
    static final int MAX_FIELDS = 20;

    public static void main(String[] args) throws Exception {
        if (args.length < 1) {
            System.err.println("uso: Debugger <ClassName> [jvmArgs]");
            System.exit(1);
        }
        String mainClass = args[0];
        String jvmArgs = args.length > 1 ? args[1] : "";

        // experimento de performance (Fase 0.5): comparar SUSPEND_ALL vs
        // SUSPEND_EVENT_THREAD, e custo de extrair locals/stack vs só contar eventos
        int suspendPolicy = "thread".equals(System.getProperty("spike.suspend"))
                ? EventRequest.SUSPEND_EVENT_THREAD
                : EventRequest.SUSPEND_ALL;
        boolean skipData = Boolean.getBoolean("spike.skipdata");
        int sampleN = Integer.getInteger("spike.sample", 1); // extrai dados só a cada N eventos; os outros só resumem
        long t0 = System.nanoTime();
        final int[] eventCount = {0};
        final int[] emittedCount = {0};
        // Post-spike scope decision (Fase 0.5, see spec.md "Throttling de
        // eventos"): 5,000 step events per execution, same cap as the C#
        // side (events::STEP_EVENT_CAP in sandbox/src/events.rs). Once hit,
        // disables the StepRequest and lets the program finish on its own
        // (or hit the external timeout) with no instrumentation overhead —
        // this is a hard cap, not sampling.
        final int STEP_EVENT_CAP = 5000;
        final StepRequest[] activeStepReq = {null};
        final boolean[] capped = {false};

        LaunchingConnector connector = Bootstrap.virtualMachineManager().defaultConnector();
        Map<String, Connector.Argument> arguments = connector.defaultArguments();
        arguments.get("main").setValue(mainClass);
        arguments.get("options").setValue(jvmArgs);

        VirtualMachine vm = connector.launch(arguments);

        redirectStream(vm.process().getInputStream(), System.out);
        redirectStream(vm.process().getErrorStream(), System.err);

        EventRequestManager erm = vm.eventRequestManager();
        ClassPrepareRequest cpr = erm.createClassPrepareRequest();
        cpr.addClassFilter(mainClass);
        cpr.setSuspendPolicy(EventRequest.SUSPEND_ALL);
        cpr.enable();

        EventQueue queue = vm.eventQueue();
        boolean running = true;
        while (running) {
            EventSet eventSet = queue.remove();
            for (Event event : eventSet) {
                if (event instanceof ClassPrepareEvent) {
                    // não dá pra criar o StepRequest aqui ainda: a thread está em
                    // bootstrap/classloading, não dentro do método main de verdade.
                    // Precisa de um breakpoint na 1ª linha de main() pra pegar a
                    // thread já posicionada dentro do método, e só então step.
                    ClassPrepareEvent cpe = (ClassPrepareEvent) event;
                    ClassType classType = (ClassType) cpe.referenceType();
                    Method mainMethod = classType.methodsByName("main").get(0);
                    Location firstLine = mainMethod.location();
                    BreakpointRequest bpReq = erm.createBreakpointRequest(firstLine);
                    bpReq.setSuspendPolicy(suspendPolicy);
                    bpReq.enable();
                } else if (event instanceof BreakpointEvent) {
                    StepRequest stepReq = erm.createStepRequest(
                            ((BreakpointEvent) event).thread(),
                            StepRequest.STEP_LINE, StepRequest.STEP_OVER);
                    stepReq.addClassExclusionFilter("java.*");
                    stepReq.addClassExclusionFilter("jdk.*");
                    stepReq.addClassExclusionFilter("sun.*");
                    stepReq.setSuspendPolicy(suspendPolicy);
                    stepReq.enable();
                    activeStepReq[0] = stepReq;
                    eventCount[0]++;
                    if (!skipData && eventCount[0] % sampleN == 0) {
                        emitStepEvent((LocatableEvent) event, t0);
                        emittedCount[0]++;
                        if (emittedCount[0] >= STEP_EVENT_CAP) {
                            capped[0] = true;
                            stepReq.disable();
                            System.out.println("{\"type\":\"step_limit_exceeded\"}");
                        }
                    }
                } else if (event instanceof StepEvent) {
                    if (capped[0]) {
                        // A JDWP request may already have an event in
                        // flight at the moment we disabled it — drop it
                        // instead of emitting/counting past the cap.
                        continue;
                    }
                    eventCount[0]++;
                    if (!skipData && eventCount[0] % sampleN == 0) {
                        emitStepEvent((StepEvent) event, t0);
                        emittedCount[0]++;
                        if (emittedCount[0] >= STEP_EVENT_CAP) {
                            capped[0] = true;
                            activeStepReq[0].disable();
                            System.out.println("{\"type\":\"step_limit_exceeded\"}");
                        }
                    }
                } else if (event instanceof VMDeathEvent || event instanceof VMDisconnectEvent) {
                    running = false;
                }
            }
            if (running) {
                vm.resume();
            }
        }

        // Architectural gap found empirically (Fase 2): LaunchingConnector
        // runs the target as a SEPARATE JVM process from this Debugger
        // process. A cgroup OOM kill lands on whichever process actually
        // holds the memory — almost always the target, not this debugger —
        // so nsjail's own exit-code/signal-based detection one level up
        // (events::run_nsjail in java.rs, which only watches nsjail's
        // DIRECT child, i.e. THIS process) never sees it: this process just
        // observes a normal VMDeathEvent and would otherwise exit 0 with no
        // trace of what happened. Confirmed by test: SIGKILLing the target
        // process directly (simulating what a cgroup OOM kill does) leaves
        // this Debugger process's own exit code untouched (0), silently
        // swallowing the failure.
        //
        // Fix: check the target's own exit value. On Linux, a process
        // killed by signal N conventionally reports 128+N here — the same
        // convention nsjail itself uses (see events.rs). Best-effort, same
        // spirit as events::run_nsjail's LikelyOom heuristic one level up:
        // exit value 137 (SIGKILL) here is attributed to memory_limit_exceeded
        // since cgroup OOM is the most likely cause, but it is NOT
        // unambiguous — RLIMIT_CPU exhaustion of just this thread (a
        // different kernel mechanism, also delivered as SIGKILL, also with
        // no nsjail-level marker) would look identical from here. Reduced
        // that specific overlap by tying java.rs's --rlimit_cpu to the same
        // configurable time_limit_secs nsjail's --time_limit uses (was
        // hardcoded to a fixed 10s before, an avoidable source of exactly
        // this ambiguity), but the two mechanisms remain distinct so a false
        // positive is still possible in principle.
        try {
            int exitValue = vm.process().exitValue();
            if (exitValue == 137) {
                System.out.println("{\"type\":\"memory_limit_exceeded\"}");
            }
        } catch (IllegalThreadStateException e) {
            // Process hasn't actually exited yet — shouldn't happen once
            // VMDeathEvent/VMDisconnectEvent has fired, but don't crash.
        }

        long elapsedMs = (System.nanoTime() - t0) / 1_000_000;
        System.err.println(String.format(
                "[perf] suspendPolicy=%s skipData=%s sampleN=%d eventosTotais=%d emitidos=%d tempo=%dms taxaTotal=%.1f ev/s taxaEmitida=%.1f ev/s",
                suspendPolicy == EventRequest.SUSPEND_ALL ? "ALL" : "EVENT_THREAD",
                skipData, sampleN, eventCount[0], emittedCount[0], elapsedMs,
                eventCount[0] * 1000.0 / Math.max(elapsedMs, 1),
                emittedCount[0] * 1000.0 / Math.max(elapsedMs, 1)));
    }

    static void emitStepEvent(LocatableEvent event, long t0) {
        try {
            ThreadReference thread = event.thread();
            StackFrame frame = thread.frame(0);
            Location loc = frame.location();

            StringBuilder locals = new StringBuilder("{");
            boolean first = true;
            try {
                for (LocalVariable var : frame.visibleVariables()) {
                    Value val = frame.getValue(var);
                    if (!first) locals.append(",");
                    first = false;
                    locals.append('"').append(var.name()).append("\":")
                            .append(serializeValue(val, MAX_DEPTH, new HashSet<>()));
                }
            } catch (AbsentInformationException e) {
                // sem debug info pra essa frame (ex: código de biblioteca)
            }
            locals.append('}');

            StringBuilder stack = new StringBuilder("[");
            List<StackFrame> frames = thread.frames();
            for (int i = 0; i < frames.size(); i++) {
                if (i > 0) stack.append(',');
                stack.append('"').append(frames.get(i).location().method().name()).append('"');
            }
            stack.append(']');

            // memory_bytes stays null: Runtime.getRuntime() here would
            // measure the debugger's OWN JVM (this process), not the
            // launched target's — that would be actively misleading, not
            // just noisy, so it's omitted until there's a real way to read
            // the debuggee's heap (JMX-over-JDWP against the target).
            System.out.println(String.format(
                    "{\"type\":\"step\",\"line\":%d,\"locals\":%s,\"stack\":%s,\"time_ns\":%d,\"memory_bytes\":null}",
                    loc.lineNumber(), locals, stack, System.nanoTime() - t0));
        } catch (Exception e) {
            System.err.println("{\"type\":\"error\",\"message\":\"" + escapeJson(String.valueOf(e)) + "\"}");
        }
    }

    // Serialização profunda de Value do JDI, com cap de profundidade/elementos/campos
    // e detecção de ciclo (visiting é por variável top-level, não compartilhado entre
    // variáveis diferentes — duas variáveis apontando pro mesmo objeto não é ciclo,
    // é só aliasing).
    static String serializeValue(Value val, int depth, Set<Long> visiting) {
        if (val == null) {
            return "null";
        }
        if (val instanceof BooleanValue) {
            return String.valueOf(((BooleanValue) val).value());
        }
        if (val instanceof CharValue) {
            return "\"" + escapeJson(String.valueOf(((CharValue) val).value())) + "\"";
        }
        if (val instanceof PrimitiveValue) {
            return val.toString(); // int/long/double/etc. — toString() já é o literal numérico
        }
        if (val instanceof StringReference) {
            return "\"" + escapeJson(((StringReference) val).value()) + "\"";
        }
        if (val instanceof ArrayReference) {
            ArrayReference arr = (ArrayReference) val;
            long id = arr.uniqueID();
            if (visiting.contains(id)) {
                return "\"<ciclo, id=" + id + ">\"";
            }
            if (depth <= 0) {
                return "\"<" + arr.type().name() + "[" + arr.length() + "], profundidade máxima>\"";
            }
            visiting.add(id);
            int cap = Math.min(arr.length(), MAX_ARRAY_ELEMENTS);
            StringBuilder sb = new StringBuilder("[");
            List<Value> values = arr.getValues(0, cap);
            for (int i = 0; i < values.size(); i++) {
                if (i > 0) sb.append(',');
                sb.append(serializeValue(values.get(i), depth - 1, visiting));
            }
            if (arr.length() > cap) {
                sb.append(",\"...(+").append(arr.length() - cap).append(" elementos)\"");
            }
            sb.append(']');
            visiting.remove(id);
            return sb.toString();
        }
        if (val instanceof ObjectReference) {
            ObjectReference obj = (ObjectReference) val;
            long id = obj.uniqueID();
            if (visiting.contains(id)) {
                return "\"<ciclo, id=" + id + ">\"";
            }
            if (depth <= 0) {
                return "\"<" + obj.referenceType().name() + ", id=" + id + ", profundidade máxima>\"";
            }
            visiting.add(id);
            List<Field> fields = obj.referenceType().allFields();
            StringBuilder sb = new StringBuilder("{");
            boolean first = true;
            int count = 0;
            for (Field f : fields) {
                if (f.isStatic()) continue;
                if (count >= MAX_FIELDS) {
                    sb.append(first ? "" : ",").append("\"...\":\"campos omitidos\"");
                    break;
                }
                Value fv;
                try {
                    fv = obj.getValue(f);
                } catch (Exception e) {
                    fv = null;
                }
                if (!first) sb.append(',');
                first = false;
                sb.append('"').append(f.name()).append("\":")
                        .append(serializeValue(fv, depth - 1, visiting));
                count++;
            }
            sb.append('}');
            visiting.remove(id);
            return sb.toString();
        }
        return "\"" + escapeJson(String.valueOf(val)) + "\"";
    }

    static String escapeJson(String s) {
        return s.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n");
    }

    static void redirectStream(InputStream in, PrintStream out) {
        Thread t = new Thread(() -> {
            try (BufferedReader reader = new BufferedReader(new InputStreamReader(in))) {
                String line;
                while ((line = reader.readLine()) != null) {
                    out.println(line);
                }
            } catch (IOException e) {
                // stream fechado quando o processo debuggee termina
            }
        });
        t.setDaemon(true);
        t.start();
    }
}
