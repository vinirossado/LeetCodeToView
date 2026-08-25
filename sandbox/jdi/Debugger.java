import com.sun.jdi.*;
import com.sun.jdi.connect.*;
import com.sun.jdi.event.*;
import com.sun.jdi.request.*;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.PrintStream;
import java.util.ArrayList;
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
    // Cap on the number of frame names serialized into a `stack` array per
    // step event. Needed ONLY after switching stepping from STEP_OVER to
    // STEP_INTO (see the StepRequest.STEP_INTO comment below): under
    // STEP_OVER, `stack` was always exactly 1 frame deep (the stepper never
    // entered a called method), so no cap was ever needed. Under STEP_INTO,
    // a deeply recursive program's `stack` grows one frame per recursive
    // call -- and since every SINGLE step event serializes the FULL stack
    // again, total emitted bytes grow QUADRATICALLY with recursion depth
    // (~stack_depth^2/2 bytes across the whole trace). Confirmed empirically
    // running test-snippets/DeepRecursion.java through the real API after
    // the STEP_INTO switch: ~1438 step events (each with a stack up to 1438
    // frames deep) blew through sandbox-runner's 10MB total-stdout cap
    // (events::OUTPUT_BYTE_CAP) mid-line, corrupting the final JSON line so
    // the clean `{"type":"output_truncated"}` marker never parsed as JSON
    // and the execution surfaced as a generic "internal sandbox error"
    // instead of a clean terminal event. Same cap style as
    // MAX_ARRAY_ELEMENTS/MAX_FIELDS above (truncate + say how much was
    // omitted, don't just silently drop data).
    static final int MAX_STACK_FRAMES = 50;

    // memory_bytes instrumentation toggle (mirrors spike.suspend/
    // spike.skipdata/spike.sample above). Defaults to ON when the property
    // isn't set at all (java.rs doesn't pass it today) -- this flag exists
    // so the A/B overhead measurement documented in tasks.md could be taken
    // against the exact same binary, not so it needs to be wired through as
    // a real feature flag.
    static boolean readMem = true;
    // Cached once (see initMemoryProbe): resolving Method objects and the
    // Runtime singleton via JDI is itself a JDWP round trip, so doing it on
    // every step would double-count overhead that has nothing to do with
    // the actual totalMemory()/freeMemory() calls we're trying to measure.
    static ObjectReference runtimeInstance;
    static Method totalMemoryMethod;
    static Method freeMemoryMethod;
    static VirtualMachine targetVm;
    // Thread count observed right after the main() breakpoint (JVM
    // housekeeping threads only, at that point) -- see readUsedMemory's
    // deadlock-avoidance guard. Caching this means the common case (thread
    // count unchanged since init) costs exactly ONE extra JDWP round trip
    // per step (vm.allThreads(), just to compare .size()), instead of also
    // calling isSystemThread() -- itself 1-2 more round trips
    // (name()/threadGroup()) -- on every housekeeping thread on every
    // single step.
    static int baselineThreadCount;

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
        readMem = System.getProperty("spike.mem") == null || Boolean.getBoolean("spike.mem");
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

        // Multi-thread event model decision (spec.md "Multi-thread", pending
        // since Fase 1): blocked in the MVP rather than modeling `stack` per
        // thread. Detected at runtime, not statically at submission time —
        // a static grep for `new Thread`/etc. would have real false
        // positives (the string/comment case) and false negatives
        // (reflection-based thread creation), so this instead watches for
        // an actual second non-JVM-internal thread starting, matching how
        // every other Fase 2 hardening signal in this file (OOM, stack
        // overflow, timeout) was done — observed real runtime behavior,
        // not a static guess.
        final ThreadReference[] mainThread = {null};

        LaunchingConnector connector = Bootstrap.virtualMachineManager().defaultConnector();
        Map<String, Connector.Argument> arguments = connector.defaultArguments();
        arguments.get("main").setValue(mainClass);
        arguments.get("options").setValue(jvmArgs);

        VirtualMachine vm = connector.launch(arguments);

        redirectStream(vm.process().getInputStream(), System.out, null);
        // Captured (not just relayed) so an uncaught exception's message/
        // stack trace can be surfaced as a clean error event below instead
        // of silently vanishing into this driver's own stderr (invisible
        // to the API/user — see the exitValue check after the event loop).
        List<String> targetStderr = new ArrayList<>();
        Thread stderrRedirect = redirectStream(vm.process().getErrorStream(), System.err, targetStderr);

        EventRequestManager erm = vm.eventRequestManager();
        ClassPrepareRequest cpr = erm.createClassPrepareRequest();
        cpr.addClassFilter(mainClass);
        cpr.setSuspendPolicy(EventRequest.SUSPEND_ALL);
        cpr.enable();

        // Must suspend (not just observe) on each thread start: the whole
        // point is to stop a second real thread before it runs any of the
        // user's code unobserved.
        ThreadStartRequest tsr = erm.createThreadStartRequest();
        tsr.setSuspendPolicy(EventRequest.SUSPEND_ALL);
        tsr.enable();

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
                } else if (event instanceof ThreadStartEvent) {
                    ThreadReference started = ((ThreadStartEvent) event).thread();
                    // The target's main thread fires its own ThreadStartEvent
                    // too, typically before the ClassPrepare/Breakpoint pair
                    // above ever runs — mainThread[0] is still null at that
                    // point, so it's naturally let through here (nothing to
                    // compare against yet) and gets recorded as `mainThread`
                    // once the BreakpointEvent below fires. JVM housekeeping
                    // threads (Reference Handler, Finalizer, Signal
                    // Dispatcher, Common-Cleaner, ...) are direct children of
                    // the "system" ThreadGroup, never of "main" — confirmed
                    // empirically (see tasks.md) rather than assumed from
                    // general JDK knowledge, since this is JDK-version-
                    // dependent territory.
                    if (mainThread[0] != null && !started.equals(mainThread[0]) && !isSystemThread(started)) {
                        System.out.println(
                                "{\"type\":\"error\",\"message\":\"multi-thread execution is not supported yet (MVP scope)\"}");
                        vm.exit(1);
                        // System.exit (not just running=false + fall through
                        // to a normal main() return) so THIS Debugger
                        // process's own exit code is non-zero — matching
                        // csharp.rs's run_worker, which returns 1 for the
                        // same block. Consistency matters here: java.rs's
                        // nsjail wrapper (events::run_nsjail) turns a
                        // non-zero exit into ProcessSandboxRunner throwing,
                        // which is what makes ExecutionJob mark the
                        // execution FAILED instead of COMPLETED. Without
                        // this, Debugger's own JVM would exit 0 normally
                        // after the loop, and the trace would show the
                        // right error EVENT but the wrong overall status.
                        // (Unreachable past this point — System.exit halts
                        // the JVM immediately, no need for `running = false`.)
                        System.exit(1);
                    }
                } else if (event instanceof BreakpointEvent) {
                    mainThread[0] = ((BreakpointEvent) event).thread();
                    if (readMem) {
                        initMemoryProbe(vm, mainThread[0]);
                    }
                    // STEP_INTO (not STEP_OVER): with STEP_OVER, the stepper never
                    // actually enters a called method's frames, so thread.frames()
                    // below always reported just the current single frame (e.g.
                    // ["main"]) no matter how deep the user's real call chain went —
                    // this was the direct cause of the call stack never showing more
                    // than one frame for Java (confirmed empirically: a recursive
                    // program's `stack` array never grew past size 1 under
                    // STEP_OVER). The class-exclusion filters below already keep
                    // STEP_INTO from diving into JVM-internal code (java.*/jdk.*/
                    // sun.*), which is what makes STEP_INTO usable here instead of
                    // drowning in JDK-internal steps.
                    StepRequest stepReq = erm.createStepRequest(
                            ((BreakpointEvent) event).thread(),
                            StepRequest.STEP_LINE, StepRequest.STEP_INTO);
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
        // Join before reading targetStderr: the redirect thread may still be
        // draining the last buffered lines right as the process exits (the
        // stream doesn't necessarily close in the same instant VMDeathEvent
        // fires) — without this, the captured list below could be read
        // mid-write, silently missing the exception's last line(s).
        try {
            stderrRedirect.join(2000);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }

        // Both branches below need to end the DRIVER's own process with a
        // non-zero exit code, not just print the right JSON — found
        // empirically testing the new branch: ProcessSandboxRunner (API
        // side) only inspects sandbox-runner's exit code to decide
        // completed vs. failed, never the JSON event content itself (that
        // only gets checked inside ExecutionJob's catch block, i.e. only
        // once an exception has already been thrown). Printing a correct
        // `{"type":"error",...}`/`{"type":"memory_limit_exceeded"}` line
        // while the driver still exits 0 is exactly the multi-thread bug
        // already fixed elsewhere in this file (`System.exit(1)` after the
        // block event) — same fix needed here, just not previously applied
        // to either of these two branches.
        boolean sawAbnormalTargetExit = false;
        try {
            // Real race found empirically while testing this exact check
            // (not present before, since the pre-existing 137/OOM case
            // apparently never hit it in practice — likely because a
            // SIGKILL reaps faster than a JVM's own exception-driven exit,
            // which does shutdown-hook/cleanup work first): `VMDeathEvent`
            // firing does NOT guarantee `vm.process().exitValue()` won't
            // still throw `IllegalThreadStateException` — the OS process
            // can still be a beat away from being reaped. A bare
            // `exitValue()` call right after the event loop reproduced
            // this consistently for an uncaught-exception exit (which
            // does more JVM-side work before actually exiting than a raw
            // SIGKILL does) — confirmed by adding a temporary debug print
            // that silently never ran, meaning the IllegalThreadStateException
            // branch below was firing every time. Fixed with a bounded
            // wait first.
            vm.process().waitFor(5, java.util.concurrent.TimeUnit.SECONDS);
            int exitValue = vm.process().exitValue();
            if (exitValue == 137) {
                System.out.println("{\"type\":\"memory_limit_exceeded\"}");
                sawAbnormalTargetExit = true;
            } else if (exitValue != 0) {
                // Genuinely new failure class found empirically (not the
                // OOM/timeout ones above): an uncaught exception in user
                // code (or the user's own `System.exit(N)`) makes the
                // TARGET exit non-zero — conventionally 1 for an uncaught
                // exception — but nothing checked for that before this,
                // so the driver fell through to its own normal exit 0 and
                // the API reported `status: "completed"` with a silently
                // truncated trace, no matter how the program actually
                // failed. Confirmed via POST /executions against the real
                // API: an uncaught ArrayIndexOutOfBoundsException produced
                // exactly that — 1 step, the pre-crash stdout, then
                // nothing. Fixed by surfacing the target's own captured
                // stderr (the actual exception message/stack trace, or
                // whatever it printed before an explicit exit) as a clean
                // error event — this is the user's own program's output,
                // not internal sandbox detail, so unlike
                // SandboxErrorSanitizer's genericization it's safe and
                // useful to show verbatim.
                String detail = String.join("\n", targetStderr);
                String message = detail.isEmpty()
                        ? "program exited with code " + exitValue
                        : detail;
                System.out.println("{\"type\":\"error\",\"message\":\"" + escapeJson(message) + "\"}");
                sawAbnormalTargetExit = true;
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

        if (sawAbnormalTargetExit) {
            // Same reasoning as the multi-thread block's System.exit(1)
            // above: ProcessSandboxRunner/ExecutionJob (API side) only
            // decide completed-vs-failed from THIS process's own exit
            // code, never from the JSON events themselves unless an
            // exception was already thrown — so printing the right event
            // above isn't enough on its own, this process must also exit
            // non-zero for the API to actually mark the execution FAILED.
            System.exit(1);
        }
    }

    // Resolves java.lang.Runtime's totalMemory()/freeMemory() Methods and
    // invokes the static Runtime.getRuntime() once to cache the singleton
    // ObjectReference, all via JDI reflection against the TARGET vm (not
    // this driver's own classpath -- Class.forName would resolve against
    // the wrong JVM entirely, same class of mistake as the old
    // Runtime.getRuntime() call this replaces). Called once, right after
    // the target's main() breakpoint fires (thread is guaranteed suspended
    // there regardless of suspendPolicy), so the per-step cost on the hot
    // path is only the two invokeMethod calls themselves, not also the
    // method/class lookup.
    static void initMemoryProbe(VirtualMachine vm, ThreadReference thread) {
        targetVm = vm;
        try {
            List<ReferenceType> classes = vm.classesByName("java.lang.Runtime");
            if (classes.isEmpty()) return; // shouldn't happen, but fail open
            ClassType runtimeClass = (ClassType) classes.get(0);
            Method getRuntimeMethod = runtimeClass.methodsByName("getRuntime", "()Ljava/lang/Runtime;").get(0);
            totalMemoryMethod = runtimeClass.methodsByName("totalMemory", "()J").get(0);
            freeMemoryMethod = runtimeClass.methodsByName("freeMemory", "()J").get(0);
            // INVOKE_SINGLE_THREADED: without it, invokeMethod resumes EVERY
            // thread in the target VM for the duration of the call (JDI
            // default), which would silently defeat SUSPEND_ALL's whole
            // purpose (a 2nd real thread could run unobserved between
            // steps) and race with the ThreadStartEvent multi-thread guard
            // above. With it, only `thread` itself is resumed to make the
            // call, everything else stays suspended exactly as SUSPEND_ALL
            // intends. Confirmed empirically (see tasks.md): without this
            // flag, MultiThread.java's second thread got a chance to start
            // between step events instead of being caught by the guard.
            Value result = runtimeClass.invokeMethod(thread, getRuntimeMethod, List.of(), ClassType.INVOKE_SINGLE_THREADED);
            runtimeInstance = (ObjectReference) result;
            baselineThreadCount = vm.allThreads().size();
        } catch (Exception e) {
            // Fail open, same tolerance as isSystemThread/serializeValue
            // elsewhere in this file: memory_bytes just stays null for this
            // run rather than crashing the whole instrumented execution
            // over a best-effort metric.
            runtimeInstance = null;
            System.err.println("[mem] falha ao inicializar sonda de memória: " + e);
        }
    }

    // Invokes Runtime.totalMemory()/freeMemory() INSIDE the target VM via
    // JDI remote method invocation (2 synchronous JDWP round trips, each at
    // least as expensive as a step round trip -- see tasks.md for measured
    // overhead) and returns totalMemory-freeMemory, i.e. actually-used heap
    // in the TARGET process. This is the whole point of doing it this way
    // instead of Runtime.getRuntime() called directly in this file: that
    // would read the DRIVER's own heap (a separate JVM process), not the
    // target's.
    static Long readUsedMemory(ThreadReference thread) {
        if (runtimeInstance == null) return null;
        // Real, reproducible deadlock found empirically (see tasks.md,
        // "memória (bytes...)"): invokeMethod (even with
        // INVOKE_SINGLE_THREADED) can permanently hang the WHOLE target VM
        // -- not just this driver's call -- when it races with a second
        // real thread starting concurrently (MultiThread.java: ~60-90% of
        // runs across two independent A/B trials, with and without
        // INVOKE_SINGLE_THREADED, confirmed to never return even after 60s,
        // not just slow). Root cause narrowed to a HotSpot/JDWP-internal
        // conflict between delivering a SUSPEND_ALL ThreadStartEvent for the
        // new thread and servicing an in-flight invoke on another thread --
        // beyond what's fixable from the driver side. vm.allThreads() is a
        // fresh JDWP query against the target's ACTUAL current thread list
        // (unlike our own event-driven bookkeeping, e.g. mainThread[0]
        // above, which can be stale by definition -- it's only updated when
        // OUR loop gets around to processing a ThreadStartEvent), so it
        // reliably observes a just-started thread even in the window before
        // its ThreadStartEvent has reached our queue.remove() loop -- the
        // exact window where the deadlock was reproduced. Skipping the
        // probe whenever more than one non-JVM-housekeeping thread is
        // currently live closes that race: validated with 30/30 clean runs
        // of MultiThread.java after adding this check (0/30 before, in the
        // same two trials above) -- the existing multi-thread guard
        // (ThreadStartEvent handler above) still catches and rejects the
        // program immediately after, unaffected by this.
        try {
            List<ThreadReference> threads = targetVm.allThreads();
            // Fast path: JVM housekeeping thread count is stable across a
            // single-threaded run, so an unchanged count (the overwhelming
            // common case -- multi-threaded programs are MVP-unsupported
            // anyway) means nothing new to check, no need to call
            // isSystemThread (itself more JDWP round trips) per thread.
            if (threads.size() > baselineThreadCount) {
                for (ThreadReference t : threads) {
                    if (!t.equals(thread) && !isSystemThread(t)) {
                        return null;
                    }
                }
            }
        } catch (Exception e) {
            return null;
        }
        try {
            long total = ((LongValue) runtimeInstance.invokeMethod(
                    thread, totalMemoryMethod, List.of(), ObjectReference.INVOKE_SINGLE_THREADED)).value();
            long free = ((LongValue) runtimeInstance.invokeMethod(
                    thread, freeMemoryMethod, List.of(), ObjectReference.INVOKE_SINGLE_THREADED)).value();
            return total - free;
        } catch (Exception e) {
            // Same fail-open reasoning as initMemoryProbe. Also covers
            // IncompatibleThreadStateException, which would fire if this
            // is ever called with a thread not actually suspended by an
            // event -- shouldn't happen given where this is called from,
            // but this metric isn't worth crashing the run over if it does.
            return null;
        }
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

            // Innermost frames first (frames.get(0) is the current frame, same
            // order thread.frames() already returns) -- capped at
            // MAX_STACK_FRAMES so a deeply recursive program doesn't blow up
            // per-event size (see MAX_STACK_FRAMES's doc comment above).
            StringBuilder stack = new StringBuilder("[");
            List<StackFrame> frames = thread.frames();
            int frameCount = Math.min(frames.size(), MAX_STACK_FRAMES);
            for (int i = 0; i < frameCount; i++) {
                if (i > 0) stack.append(',');
                stack.append('"').append(frames.get(i).location().method().name()).append('"');
            }
            if (frames.size() > MAX_STACK_FRAMES) {
                stack.append(",\"...(+").append(frames.size() - MAX_STACK_FRAMES).append(" frames)\"");
            }
            stack.append(']');

            // memory_bytes: read via JDI remote method invocation against
            // the TARGET vm's own Runtime.totalMemory()/freeMemory() (see
            // initMemoryProbe/readUsedMemory) -- NOT Runtime.getRuntime()
            // called directly in this file, which would measure the
            // debugger's OWN JVM (a separate process), not the launched
            // target's.
            Long memBytes = readMem ? readUsedMemory(thread) : null;
            String memJson = memBytes == null ? "null" : String.valueOf(memBytes);
            System.out.println(String.format(
                    "{\"type\":\"step\",\"line\":%d,\"locals\":%s,\"stack\":%s,\"time_ns\":%d,\"memory_bytes\":%s}",
                    loc.lineNumber(), locals, stack, System.nanoTime() - t0, memJson));
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

    // Empirically confirmed inside this project's actual sandboxed JDK
    // (`sandbox/Dockerfile`'s openjdk-17-jdk-headless — the standalone
    // spike image), not assumed from general JDK knowledge: a genuinely
    // single-threaded program (Loop.java)
    // still starts "Notification Thread" (group "system") AND
    // "Common-Cleaner" (group "InnocuousThreadGroup" — NOT "system", the
    // first version of this check got that wrong and false-positived on
    // every single-threaded program). Combining a group-name check with a
    // small name allowlist as defense-in-depth for other well-known JVM
    // housekeeping threads not observed in that one test program (older
    // JDKs' "Reference Handler"/"Finalizer", "Signal Dispatcher",
    // "Attach Listener" if a profiler attaches, "DestroyJavaVM" during
    // shutdown). Best-effort, not exhaustively proven for every possible
    // program — same tolerance this codebase already accepts for the
    // OOM/stack-overflow heuristics elsewhere in this file.
    //
    // NOTE on the discrepancy between this JDK 17 test environment and
    // production/docker-compose: the combined API image (Dockerfile.api,
    // .ci/Dockerfile) was found to actually run sandboxed code on JDK 25,
    // not JDK 17 as its comments used to (incorrectly) claim — see
    // Dockerfile.api's comment on the JDK install. This group-name/allowlist
    // check is still confirmed working there too, not just here: the E2E
    // suite's "blocks Java code that spawns a real thread" test runs
    // against that exact image (docker-compose) and passes consistently.
    private static final Set<String> KNOWN_JVM_THREAD_NAMES = Set.of(
            "Reference Handler", "Finalizer", "Signal Dispatcher",
            "Attach Listener", "DestroyJavaVM", "process reaper");

    static boolean isSystemThread(ThreadReference thread) {
        try {
            if (KNOWN_JVM_THREAD_NAMES.contains(thread.name())) {
                return true;
            }
            ThreadGroupReference group = thread.threadGroup();
            return group == null
                    || "system".equals(group.name())
                    || "InnocuousThreadGroup".equals(group.name());
        } catch (Exception e) {
            return true;
        }
    }

    /**
     * Real bug found and fixed while adding the uncaught-exception error
     * event below: a Java stack trace's continuation lines start with a
     * literal TAB character ({@code \tat Main.main(...)}), which this
     * method used to pass through unescaped. A raw control character
     * (anything below U+0020) inside a JSON string is invalid per the spec
     * (RFC 8259) — Jackson's {@code ObjectMapper.readTree} (the API side,
     * {@code ExecutionJob.parseEventOrStdout}) correctly rejects it, so the
     * whole intended error event silently fell through to being wrapped as
     * a raw stdout-text event instead of being recognized as {@code
     * "type":"error"} — confirmed via a real POST /executions with an
     * uncaught exception, not assumed. Escapes every control character
     * generically (not just the two JSON has short escapes for, {@code \n}/
     * {@code \t}) so this can't recur for any other stack-trace/toString()
     * content this method is used for elsewhere in this file.
     */
    static String escapeJson(String s) {
        StringBuilder sb = new StringBuilder(s.length());
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '\\' -> sb.append("\\\\");
                case '"' -> sb.append("\\\"");
                case '\n' -> sb.append("\\n");
                case '\r' -> sb.append("\\r");
                case '\t' -> sb.append("\\t");
                default -> {
                    if (c < 0x20) {
                        sb.append(String.format("\\u%04x", (int) c));
                    } else {
                        sb.append(c);
                    }
                }
            }
        }
        return sb.toString();
    }

    /**
     * Relays {@code in} to {@code out} line by line, as before. {@code capture},
     * when non-null, ALSO appends every line to that list (in addition to the
     * relay) — used for the target's stderr so its content is still available
     * for inspection after the process exits, not just visible live on this
     * driver's own stderr (which the API/frontend never see). Returns the
     * relay thread so callers needing the capture can {@link Thread#join()} it
     * first, to avoid reading a partially-drained list.
     */
    static Thread redirectStream(InputStream in, PrintStream out, List<String> capture) {
        Thread t = new Thread(() -> {
            try (BufferedReader reader = new BufferedReader(new InputStreamReader(in))) {
                String line;
                while ((line = reader.readLine()) != null) {
                    out.println(line);
                    if (capture != null) {
                        capture.add(line);
                    }
                }
            } catch (IOException e) {
                // stream fechado quando o processo debuggee termina
            }
        });
        t.setDaemon(true);
        t.start();
        return t;
    }
}
