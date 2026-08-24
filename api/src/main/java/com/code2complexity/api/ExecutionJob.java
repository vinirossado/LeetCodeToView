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
            // Never forward a raw exception message to the client as-is —
            // it can come straight from sandbox-runner/dotnet/nsjail and
            // leak host paths, panic locations, etc. See
            // SandboxErrorSanitizer for what is/isn't safe to pass through.
            String sanitized = SandboxErrorSanitizer.sanitize(e.getMessage(), "execution " + execution.getId(), e);
            ObjectNode errorEvent = MAPPER.createObjectNode();
            errorEvent.put("type", "error");
            errorEvent.put("message", sanitized);
            store.appendEvent(execution.getId(), errorEvent);
            store.finish(execution.getId(), ExecutionStatus.FAILED);
        }
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
