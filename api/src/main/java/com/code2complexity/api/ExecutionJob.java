package com.code2complexity.api;

import com.code2complexity.api.error.SandboxErrorSanitizer;
import com.code2complexity.api.model.Execution;
import com.code2complexity.api.model.ExecutionStatus;
import com.code2complexity.api.sandbox.SandboxRunner;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import java.util.Set;

/**
 * Runs one execution against a {@link SandboxRunner}, feeding every
 * produced line into the store as it arrives, and marking the execution
 * completed/failed at the end.
 *
 * <p>sandbox-runner's own stdout is a mix of the JSON event lines
 * (sandbox/src/events.rs) AND the sandboxed program's real stdout,
 * interleaved on the same stream (java.rs/csharp.rs run the target with
 * {@code Stdio::inherit()}). A line that isn't a JSON object with a
 * "type" field is therefore program output, not a malformed event — it's
 * wrapped into a synthetic {@code {"type":"stdout","text":...}} event
 * instead of being (wrongly) treated as a fatal parse error. This
 * "stdout" type is an API-side addition on top of the Rust event schema,
 * not something sandbox-runner itself emits.
 */
@ApplicationScoped
public class ExecutionJob {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Inject
    ExecutionStore store;

    @Inject
    SandboxRunner runner;

    public void perform(Execution execution) {
        store.updateStatus(execution.getId(), ExecutionStatus.RUNNING);
        try {
            runner.run(execution, line -> store.appendEvent(execution.getId(), parseEventOrStdout(line)));
            store.finish(execution.getId(), ExecutionStatus.COMPLETED);
        } catch (Exception e) {
            // sandbox-runner/java.rs/csharp.rs can themselves emit a
            // specific, already-clean terminal event on stdout BEFORE
            // exiting non-zero — not just the multi-thread block's
            // {"type":"error",...} this guard originally covered, but also
            // {"type":"timeout"}/"memory_limit_exceeded"/"output_truncated"/
            // "stack_overflow"/"step_limit_exceeded" (events.rs::run_nsjail,
            // called from both languages): a killed-by-nsjail process
            // reports the SAME non-zero/SIGKILL exit code up through
            // sandbox-runner regardless of which safety limit triggered it,
            // so ProcessSandboxRunner's exit-code check throws here every
            // time too. Appending a second, generic sanitized error on top
            // in any of those cases buries the specific, already-informative
            // event under a useless one — the last event is what the
            // frontend actually displays (TraceStoreService.terminalEvent(),
            // which already renders a dedicated message for every one of
            // these types, see frontend's terminalEventMessage()). Found by
            // testing a real timeout end-to-end through the API (not just
            // sandbox-runner in isolation, which is how this was originally
            // validated in Fase 2) — only fall back to the generic
            // sanitized message when nothing more specific was already
            // emitted in-band.
            if (!lastEventIsAlreadyTerminal(execution)) {
                String sanitized = SandboxErrorSanitizer.sanitize(e.getMessage(), "execution " + execution.getId(), e);
                ObjectNode errorEvent = MAPPER.createObjectNode();
                errorEvent.put("type", "error");
                errorEvent.put("message", sanitized);
                store.appendEvent(execution.getId(), errorEvent);
            }
            store.finish(execution.getId(), ExecutionStatus.FAILED);
        }
    }

    // Mirrors frontend/src/app/core/models/execution-event.model.ts's
    // TERMINAL_EVENT_TYPES (minus "error", handled by the same set here too
    // since it's just as "already specific" as the others) — keep in sync
    // if a new terminal event type is ever added to either side.
    private static final Set<String> ALREADY_TERMINAL_EVENT_TYPES =
            Set.of("error", "timeout", "memory_limit_exceeded", "output_truncated", "stack_overflow", "step_limit_exceeded");

    private static boolean lastEventIsAlreadyTerminal(Execution execution) {
        var events = execution.getEvents();
        if (events.isEmpty()) {
            return false;
        }
        JsonNode last = events.get(events.size() - 1);
        return last.isObject() && ALREADY_TERMINAL_EVENT_TYPES.contains(last.path("type").asText(null));
    }

    private static JsonNode parseEventOrStdout(String line) {
        try {
            JsonNode parsed = MAPPER.readTree(line);
            if (parsed.isObject() && parsed.has("type")) {
                return parsed;
            }
        } catch (Exception notJson) {
            // falls through — treated as raw program stdout below
        }
        ObjectNode stdout = MAPPER.createObjectNode();
        stdout.put("type", "stdout");
        stdout.put("text", line);
        return stdout;
    }
}
