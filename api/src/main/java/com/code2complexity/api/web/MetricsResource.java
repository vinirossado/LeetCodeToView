package com.code2complexity.api.web;

import com.code2complexity.api.metrics.Metrics;
import jakarta.inject.Inject;
import jakarta.ws.rs.GET;
import jakarta.ws.rs.Path;
import jakarta.ws.rs.Produces;
import jakarta.ws.rs.core.MediaType;
import jakarta.ws.rs.core.Response;

/**
 * Internal, ops/debugging-only endpoint — deliberately NOT a documented
 * public product surface (see tasks.md, "Métricas de uso e
 * observabilidade": the "no Swagger/OpenAPI, no hand-written API markdown"
 * rule applies to the actual product API, not to this kind of internal
 * tooling, but the {@code /internal/} prefix itself is the signal here:
 * nothing links to this from the frontend, and it's not meant to be a
 * stable, versioned contract for external consumers).
 *
 * <p>Returns a point-in-time snapshot of the in-memory counters in
 * {@link Metrics} — see that class for what's tracked and why it's a
 * hand-rolled counter instead of Micrometer/Prometheus.
 *
 * <p>Intentionally not covered by {@link com.code2complexity.api.ratelimit.RateLimitingFilter}:
 * it's a cheap in-memory read (no subprocess, no sandbox), same category
 * as {@code GET /executions/:id/trace}, which that filter also skips.
 */
@Path("/internal/metrics")
public class MetricsResource {

    @Inject
    Metrics metrics;

    @GET
    @Produces(MediaType.APPLICATION_JSON)
    public Response metrics() {
        return Response.ok(metrics.snapshot()).build();
    }
}
