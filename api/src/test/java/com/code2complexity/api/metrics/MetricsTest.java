package com.code2complexity.api.metrics;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;

// Plain unit test (no @QuarkusTest) — Metrics/DurationSampler have no
// CDI-specific behavior beyond being a bean, same rationale already used
// by ExecutionStoreTest.
class MetricsTest {

    @Nested
    @DisplayName("recordExecution")
    class RecordExecution {

        @Test
        @DisplayName("counts by language and status independently")
        void countsByLanguageAndStatus() {
            Metrics metrics = new Metrics();

            metrics.recordExecution("java", "completed", null, 10);
            metrics.recordExecution("java", "completed", null, 20);
            metrics.recordExecution("csharp", "failed", "timeout", 30);

            Metrics.Snapshot snapshot = metrics.snapshot();
            assertEquals(2L, snapshot.executionsByLanguage().get("java"));
            assertEquals(1L, snapshot.executionsByLanguage().get("csharp"));
            assertEquals(2L, snapshot.executionsByStatus().get("completed"));
            assertEquals(1L, snapshot.executionsByStatus().get("failed"));
        }

        @Test
        @DisplayName("only counts a terminal event type when one is given (failed executions)")
        void terminalEventTypeOnlyForFailed() {
            Metrics metrics = new Metrics();

            metrics.recordExecution("java", "completed", null, 10);
            metrics.recordExecution("java", "failed", "error", 10);
            metrics.recordExecution("java", "failed", "timeout", 10);
            metrics.recordExecution("java", "failed", "timeout", 10);

            Metrics.Snapshot snapshot = metrics.snapshot();
            assertEquals(1L, snapshot.executionsByTerminalEvent().get("error"));
            assertEquals(2L, snapshot.executionsByTerminalEvent().get("timeout"));
            // The completed execution contributed no terminal event entry.
            assertEquals(3, snapshot.executionsByTerminalEvent().values().stream().mapToLong(Long::longValue).sum());
        }

        @Test
        @DisplayName("computes average duration across all recorded executions")
        void computesAverageDuration() {
            Metrics metrics = new Metrics();

            metrics.recordExecution("java", "completed", null, 100);
            metrics.recordExecution("java", "completed", null, 200);
            metrics.recordExecution("java", "completed", null, 300);

            Metrics.Snapshot snapshot = metrics.snapshot();
            assertEquals(3L, snapshot.executionDuration().count());
            assertEquals(200.0, snapshot.executionDuration().averageMs(), 0.001);
        }
    }

    @Nested
    @DisplayName("recordAnalysis")
    class RecordAnalysis {

        @Test
        @DisplayName("counts by language and success/failure independently")
        void countsByLanguageAndOutcome() {
            Metrics metrics = new Metrics();

            metrics.recordAnalysis("java", true);
            metrics.recordAnalysis("java", true);
            metrics.recordAnalysis("csharp", false);

            Metrics.Snapshot snapshot = metrics.snapshot();
            assertEquals(2L, snapshot.analysisByLanguage().get("java"));
            assertEquals(1L, snapshot.analysisByLanguage().get("csharp"));
            assertEquals(2L, snapshot.analysisByOutcome().get("success"));
            assertEquals(1L, snapshot.analysisByOutcome().get("failure"));
        }
    }

    @Nested
    @DisplayName("snapshot")
    class Snapshot {

        @Test
        @DisplayName("returns an empty snapshot before anything is recorded")
        void emptyBeforeAnyRecording() {
            Metrics metrics = new Metrics();

            Metrics.Snapshot snapshot = metrics.snapshot();
            assertTrue(snapshot.executionsByLanguage().isEmpty());
            assertTrue(snapshot.executionsByStatus().isEmpty());
            assertTrue(snapshot.executionsByTerminalEvent().isEmpty());
            assertEquals(0L, snapshot.executionDuration().count());
            assertTrue(snapshot.analysisByLanguage().isEmpty());
            assertTrue(snapshot.analysisByOutcome().isEmpty());
        }
    }

    @Nested
    @DisplayName("reset")
    class Reset {

        @Test
        @DisplayName("clears all counters back to empty")
        void clearsEverything() {
            Metrics metrics = new Metrics();
            metrics.recordExecution("java", "completed", null, 10);
            metrics.recordAnalysis("java", true);

            metrics.reset();

            Metrics.Snapshot snapshot = metrics.snapshot();
            assertTrue(snapshot.executionsByLanguage().isEmpty());
            assertTrue(snapshot.analysisByLanguage().isEmpty());
            assertEquals(0L, snapshot.executionDuration().count());
        }
    }

    @Nested
    @DisplayName("DurationSampler")
    class DurationSamplerTests {

        @Test
        @DisplayName("computes an exact p95 for a small, known sample set")
        void computesP95ForSmallSampleSet() {
            DurationSampler sampler = new DurationSampler();
            // 100 samples: 1..100 ms. The 95th percentile of this set is 95.
            for (int i = 1; i <= 100; i++) {
                sampler.record(i);
            }

            DurationSampler.Snapshot snapshot = sampler.snapshot();
            assertEquals(100L, snapshot.count());
            assertEquals(50.5, snapshot.averageMs(), 0.001);
            assertEquals(95L, snapshot.p95Ms());
        }

        @Test
        @DisplayName("average stays exact even after the ring buffer capacity is exceeded")
        void averageStaysExactBeyondCapacity() {
            DurationSampler sampler = new DurationSampler();
            // Ring buffer capacity is 1000 — record well beyond that, all
            // the same value, so the true average is trivially known
            // regardless of which samples got evicted from the buffer.
            for (int i = 0; i < 2500; i++) {
                sampler.record(42);
            }

            DurationSampler.Snapshot snapshot = sampler.snapshot();
            assertEquals(2500L, snapshot.count());
            assertEquals(42.0, snapshot.averageMs(), 0.001);
            assertEquals(42L, snapshot.p95Ms());
        }

        @Test
        @DisplayName("reports zero count/average/p95 when nothing was recorded")
        void emptySampler() {
            DurationSampler sampler = new DurationSampler();

            DurationSampler.Snapshot snapshot = sampler.snapshot();
            assertEquals(0L, snapshot.count());
            assertEquals(0.0, snapshot.averageMs());
            assertEquals(0L, snapshot.p95Ms());
        }
    }
}
