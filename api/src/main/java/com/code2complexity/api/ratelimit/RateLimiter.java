package com.code2complexity.api.ratelimit;

import jakarta.enterprise.context.ApplicationScoped;
import java.time.Clock;
import java.util.ArrayDeque;
import java.util.Deque;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Simple in-memory sliding-window rate limiter, keyed by an arbitrary
 * caller-supplied string (e.g. {@code "executions|203.0.113.5"}).
 *
 * <p>Same "in-memory is fine for MVP" philosophy already used by
 * {@link com.code2complexity.api.ExecutionStore} — see spec.md/tasks.md,
 * "Fila/estado de execuções em memória (ou Redis) para MVP". No Redis, no
 * external dependency: a single API instance is the deploy target for now
 * (see tasks.md, backend {@code replicas: 1} in {@code .ci/stack.yml} —
 * concentrating nsjail's elevated host privilege in one instance is a
 * deliberate choice there too).
 *
 * <p><b>Known limitation:</b> counters are per-JVM — they reset on
 * restart and are NOT shared across replicas if this API is ever scaled
 * horizontally. Acceptable for a single-instance MVP; revisit (e.g. a
 * shared store) if/when horizontal scaling happens.
 */
@ApplicationScoped
public class RateLimiter {

    // One timestamp deque per key. Grows by one entry per distinct key
    // ever seen and is never fully evicted (an idle key's deque just sits
    // there empty after its timestamps age out) — bounded by the number
    // of distinct (bucket, IP) pairs seen over the process lifetime, which
    // is fine at MVP scale but would want an eviction sweep under
    // sustained high-cardinality abuse (e.g. spoofed X-Forwarded-For
    // values — see RateLimitingFilter's own caveat about that header).
    private final ConcurrentHashMap<String, Deque<Long>> requestTimestamps = new ConcurrentHashMap<>();

    private final Clock clock;

    public RateLimiter() {
        this(Clock.systemUTC());
    }

    // Package-visible so tests can inject a controllable clock instead of
    // sleeping real wall-clock seconds to exercise window expiry.
    RateLimiter(Clock clock) {
        this.clock = clock;
    }

    /**
     * Records one request for {@code key} if it is within
     * {@code maxRequests} in the trailing {@code windowSeconds}.
     *
     * @return {@code true} if the request is allowed (and now recorded);
     *         {@code false} if the caller is over the limit (nothing is
     *         recorded for a rejected call, so it doesn't itself count
     *         towards the window).
     */
    public boolean tryAcquire(String key, int maxRequests, int windowSeconds) {
        long now = clock.millis();
        long windowStart = now - (windowSeconds * 1000L);

        Deque<Long> timestamps = requestTimestamps.computeIfAbsent(key, unused -> new ArrayDeque<>());
        synchronized (timestamps) {
            while (!timestamps.isEmpty() && timestamps.peekFirst() < windowStart) {
                timestamps.pollFirst();
            }
            if (timestamps.size() >= maxRequests) {
                return false;
            }
            timestamps.addLast(now);
            return true;
        }
    }

    /**
     * Test-only reset: {@code @QuarkusTest} reuses the same application
     * (and therefore the same singleton {@link RateLimiter}) across test
     * methods within a class, so counters need to be cleared between
     * tests to avoid one test's requests bleeding into the next.
     */
    public void reset() {
        requestTimestamps.clear();
    }
}
