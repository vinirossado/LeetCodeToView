package com.code2complexity.api;

import com.code2complexity.api.error.SandboxErrorSanitizer;
import com.code2complexity.api.metrics.Metrics;
import com.code2complexity.api.model.Execution;
import com.code2complexity.api.model.ExecutionStatus;
import com.code2complexity.api.sandbox.SandboxRunner;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import io.quarkus.logging.Log;
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

    @Inject
    Metrics metrics;

    public void perform(Execution execution) {
        store.updateStatus(execution.getId(), ExecutionStatus.RUNNING);
        // Wall-clock, not CPU time — this is meant to answer "how long did
        // the caller wait for a result", which is what matters for the
        // metrics endpoint's average/p95, not how much of that was actual
        // sandboxed-process CPU vs. process-spawn/compile overhead.
        long startedAtMs = System.currentTimeMillis();
        try {
            runner.run(execution, line -> store.appendEvent(execution.getId(), parseEventOrStdout(line)));
            store.finish(execution.getId(), ExecutionStatus.COMPLETED);
            logAndRecordOutcome(execution, ExecutionStatus.COMPLETED, null, System.currentTimeMillis() - startedAtMs);
            return;
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
            logAndRecordOutcome(execution, ExecutionStatus.FAILED, terminalEventType(execution), System.currentTimeMillis() - startedAtMs);
        }
    }

    // Structured, grep-able outcome line for every execution — the whole
    // point of tasks.md's "Métricas de uso e observabilidade" item. Always
    // includes execution_id explicitly (rather than via MDC/logging
    // context) so grepping a specific id in production logs surfaces this
    // line alongside the SandboxErrorSanitizer.sanitize(...) warn line
    // above (which already includes "execution " + id in its own message)
    // — same convention, just extended here to the outcome itself, not
    // only to the failure-detail line.
    private void logAndRecordOutcome(Execution execution, ExecutionStatus status, String terminalEventType, long durationMs) {
        Log.infof(
                "execution finished execution_id=%s language=%s status=%s event=%s duration_ms=%d",
                execution.getId(), execution.getLanguage(), status.jsonValue(),
                terminalEventType == null ? "none" : terminalEventType, durationMs);
        metrics.recordExecution(execution.getLanguage(), status.jsonValue(), terminalEventType, durationMs);
    }

    // Only called for a FAILED execution. The catch block above guarantees
    // the last event is always one of ALREADY_TERMINAL_EVENT_TYPES by this
    // point: either sandbox-runner emitted a specific one in-band, or the
    // generic {"type":"error",...} fallback was just appended above when it
    // hadn't. The empty-events / non-matching-type fallback to "unknown"
    // is defensive only — not expected to be hit in practice, but cheap
    // insurance against ever NPE-ing/mis-tagging a metric on a future
    // change to the catch block's own logic.
    private static String terminalEventType(Execution execution) {
        var events = execution.getEvents();
        if (events.isEmpty()) {
            return "unknown";
        }
        JsonNode last = events.get(events.size() - 1);
        String type = last.isObject() ? last.path("type").asText(null) : null;
        return type != null ? type : "unknown";
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
