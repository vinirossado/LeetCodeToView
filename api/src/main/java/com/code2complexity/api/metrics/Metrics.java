package com.code2complexity.api.metrics;

import jakarta.enterprise.context.ApplicationScoped;
import java.util.Map;
import java.util.TreeMap;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.LongAdder;

/**
 * In-memory usage counters for {@code POST /executions} and
 * {@code POST /analysis}, exposed read-only via
 * {@link com.code2complexity.api.web.MetricsResource} ({@code GET
 * /internal/metrics}).
 *
 * <p>Same "in-memory is fine for MVP" philosophy already used by
 * {@link com.code2complexity.api.ExecutionStore} / {@link
 * com.code2complexity.api.ratelimit.RateLimiter} (see tasks.md, "Fila/
 * estado de execuções em memória (ou Redis) para MVP"): a hand-rolled
 * {@link ConcurrentHashMap}-of-{@link LongAdder} counter, no Micrometer/
 * Prometheus dependency. {@code quarkus-micrometer} is NOT currently a
 * dependency of this module (checked api/pom.xml before writing this), and
 * wiring it in cleanly (a registry, a metrics binder, choosing an export
 * format) is meaningfully more machinery than this MVP-scale "how many
 * executions ran and how did they end" need justifies — same single-JVM,
 * resets-on-restart, not-shared-across-replicas caveat as RateLimiter
 * applies here too.
 */
@ApplicationScoped
public class Metrics {

    private final ConcurrentHashMap<String, LongAdder> executionsByLanguage = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, LongAdder> executionsByStatus = new ConcurrentHashMap<>();
    // Only populated for FAILED executions — see ExecutionJob: the last
    // event of a failed execution is always one of the terminal event
    // types (error/timeout/memory_limit_exceeded/stack_overflow/
    // step_limit_exceeded/output_truncated), either emitted in-band by
    // sandbox-runner or the generic fallback ExecutionJob appends itself.
    private final ConcurrentHashMap<String, LongAdder> executionsByTerminalEvent = new ConcurrentHashMap<>();
    private final DurationSampler executionDurations = new DurationSampler();

    private final ConcurrentHashMap<String, LongAdder> analysisByLanguage = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, LongAdder> analysisByOutcome = new ConcurrentHashMap<>();

    /**
     * @param terminalEventType only meaningful (non-null) for a FAILED
     *                          execution; null for COMPLETED.
     */
    public void recordExecution(String language, String status, String terminalEventType, long durationMs) {
        increment(executionsByLanguage, language);
        increment(executionsByStatus, status);
        if (terminalEventType != null) {
            increment(executionsByTerminalEvent, terminalEventType);
        }
        executionDurations.record(durationMs);
    }

    public void recordAnalysis(String language, boolean success) {
        increment(analysisByLanguage, language);
        increment(analysisByOutcome, success ? "success" : "failure");
    }

    public Snapshot snapshot() {
        return new Snapshot(
                toMap(executionsByLanguage),
                toMap(executionsByStatus),
                toMap(executionsByTerminalEvent),
                executionDurations.snapshot(),
                toMap(analysisByLanguage),
                toMap(analysisByOutcome));
    }

    /**
     * Test-only reset: mirrors {@link com.code2complexity.api.ratelimit.RateLimiter#reset()}
     * — @QuarkusTest reuses the same singleton across test methods within a
     * class, so counters need to be cleared between tests.
     */
    public void reset() {
        executionsByLanguage.clear();
        executionsByStatus.clear();
        executionsByTerminalEvent.clear();
        executionDurations.reset();
        analysisByLanguage.clear();
        analysisByOutcome.clear();
    }

    private static void increment(ConcurrentHashMap<String, LongAdder> map, String key) {
        map.computeIfAbsent(key, unused -> new LongAdder()).increment();
    }

    // TreeMap for a deterministic (alphabetical) key order in the JSON
    // response — purely cosmetic, makes curl/eyeball comparisons stable.
    private static Map<String, Long> toMap(ConcurrentHashMap<String, LongAdder> counters) {
        Map<String, Long> result = new TreeMap<>();
        counters.forEach((key, adder) -> result.put(key, adder.sum()));
        return result;
    }

    public record Snapshot(
            Map<String, Long> executionsByLanguage,
            Map<String, Long> executionsByStatus,
            Map<String, Long> executionsByTerminalEvent,
            DurationSampler.Snapshot executionDuration,
            Map<String, Long> analysisByLanguage,
            Map<String, Long> analysisByOutcome) {
    }
}
