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
            // sandbox-runner can itself emit a specific, already-clean
            // `{"type":"error",...}` event on stdout BEFORE exiting
            // non-zero (e.g. jdi/Debugger.java's/com.rs's multi-thread
            // block: it prints a clear message and only then exits with a
            // non-zero code, specifically so this catch block also runs
            // and marks the execution FAILED). Appending a second, generic
            // sanitized error on top in that case buries the useful
            // message under a useless one — the last event is what the
            // frontend actually displays (TraceStoreService.terminalEvent()).
            // Only fall back to the generic sanitized message when nothing
            // more specific was already emitted in-band.
            if (!lastEventIsError(execution)) {
                String sanitized = SandboxErrorSanitizer.sanitize(e.getMessage(), "execution " + execution.getId(), e);
                ObjectNode errorEvent = MAPPER.createObjectNode();
                errorEvent.put("type", "error");
                errorEvent.put("message", sanitized);
                store.appendEvent(execution.getId(), errorEvent);
            }
            store.finish(execution.getId(), ExecutionStatus.FAILED);
        }
    }

    private static boolean lastEventIsError(Execution execution) {
        var events = execution.getEvents();
        if (events.isEmpty()) {
            return false;
        }
        JsonNode last = events.get(events.size() - 1);
        return last.isObject() && "error".equals(last.path("type").asText(null));
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
