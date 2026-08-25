package com.code2complexity.api.metrics;

import java.util.Arrays;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.LongAdder;

/**
 * Fixed-capacity, thread-safe sampler used to compute a cheap average/p95
 * of duration measurements (milliseconds) without unbounded memory growth.
 *
 * <p>Same "simple in-memory is fine for MVP" philosophy already used by
 * {@link com.code2complexity.api.ratelimit.RateLimiter} /
 * {@link com.code2complexity.api.ExecutionStore} — no Micrometer/Prometheus
 * histogram, just a small ring buffer guarded by a single lock (recording a
 * duration is cheap and infrequent relative to request handling, so lock
 * contention here is not a real concern).
 *
 * <p><b>Average</b> is computed over ALL recorded samples ever seen
 * (independent {@link LongAdder} count/sum), so it stays exact even after
 * older samples are evicted from the ring buffer. <b>p95</b> is only
 * approximate: it is computed from whatever is currently in the ring
 * buffer, which is capped at {@link #CAPACITY} entries — under sustained
 * load the buffer holds a mix of samples overwritten at different times
 * rather than strictly "the most recent CAPACITY", but this is good enough
 * for an ops/debugging metrics endpoint, not a billing-grade SLO tool.
 */
public class DurationSampler {

    private static final int CAPACITY = 1000;

    private final long[] samples = new long[CAPACITY];
    private final Object lock = new Object();
    private long writeIndex = 0;

    private final LongAdder count = new LongAdder();
    private final AtomicLong sum = new AtomicLong();

    public void record(long durationMs) {
        count.increment();
        sum.addAndGet(durationMs);
        synchronized (lock) {
            samples[(int) (writeIndex % CAPACITY)] = durationMs;
            writeIndex++;
        }
    }

    public Snapshot snapshot() {
        long[] copy;
        long filled;
        synchronized (lock) {
            filled = Math.min(writeIndex, CAPACITY);
            copy = Arrays.copyOf(samples, (int) filled);
        }
        Arrays.sort(copy);

        long totalCount = count.sum();
        double average = totalCount == 0 ? 0.0 : (double) sum.get() / totalCount;
        // p95: index of the smallest value at or above the 95th percentile,
        // clamped so a small sample size never indexes out of bounds.
        long p95 = 0;
        if (copy.length > 0) {
            int index = (int) Math.min(copy.length - 1, Math.ceil(copy.length * 0.95) - 1);
            p95 = copy[Math.max(index, 0)];
        }
        return new Snapshot(totalCount, average, p95);
    }

    /**
     * Test-only reset: mirrors {@link com.code2complexity.api.ratelimit.RateLimiter#reset()}
     * — @QuarkusTest reuses the same singleton across test methods.
     */
    public void reset() {
        synchronized (lock) {
            Arrays.fill(samples, 0);
            writeIndex = 0;
        }
        count.reset();
        sum.set(0);
    }

    public record Snapshot(long count, double averageMs, long p95Ms) {
    }
}
