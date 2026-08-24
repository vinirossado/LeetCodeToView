package com.code2complexity.api.ratelimit;

import com.code2complexity.api.web.ErrorResponse;
import io.vertx.core.http.HttpServerRequest;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import jakarta.ws.rs.container.ContainerRequestContext;
import jakarta.ws.rs.container.ContainerRequestFilter;
import jakarta.ws.rs.core.Context;
import jakarta.ws.rs.core.MediaType;
import jakarta.ws.rs.core.Response;
import jakarta.ws.rs.ext.Provider;
import org.eclipse.microprofile.config.inject.ConfigProperty;

/**
 * Per-IP rate limiting for the two endpoints that can tie up real CPU or
 * an nsjail/dotnet-build slot: {@code POST /executions} (runs untrusted
 * code in the sandbox) and {@code POST /analysis} (shells out to
 * static-analyzer, cheaper but still a subprocess per request). See
 * spec.md, "Isolamento de rede não impede abuso de CPU... Rate limiting
 * por IP/conta é necessário desde o MVP" and tasks.md Fase 2.
 *
 * <p>There's no auth yet (tasks.md Fase 4 is still unstarted), so this is
 * per-IP only, as the task item itself allows ("por usuário/IP"). {@code
 * GET /executions/:id/trace} and the WebSocket events endpoint are cheap
 * reads/streams and intentionally not covered.
 *
 * <p>Path-matched (checking {@code getUriInfo().getPath()} + HTTP method)
 * rather than annotation-bound to a custom {@code @RateLimited}
 * meta-annotation — both are reasonable per this task's own framing;
 * path-matching is simpler here since there are exactly two endpoints to
 * cover and no broader filter-ordering concerns in this API.
 *
 * <h2>Client IP: trusting X-Forwarded-For</h2>
 * In the docker-compose/.ci deploy topology, nginx is the only thing that
 * can reach the API container's HTTP port directly (see
 * frontend/nginx.conf / .ci/nginx.frontend.conf: no host port is
 * published for the {@code api} service, only for the reverse proxy) —
 * so in that topology, trusting a forwarded-IP header at all is
 * reasonable in principle. <b>Caveat found while implementing this,
 * checked for real rather than assumed:</b> neither nginx config
 * currently sets/overwrites {@code X-Forwarded-For} (no
 * {@code proxy_set_header X-Forwarded-For ...} line in either file) —
 * nginx's default {@code proxy_pass} forwards a client-supplied header
 * value through completely unchanged. That means, as configured *today*,
 * a client can still spoof this header and dodge the limiter entirely.
 * Falling back to the raw remote address instead doesn't fix this either:
 * every request reaching the API through nginx would then show the SAME
 * remote address (nginx's own container IP on the compose network),
 * collapsing "per-IP" into one shared bucket for every real user behind
 * the proxy. Trusting the header is still the better of the two flawed
 * options (it at least isolates well-behaved clients from each other, and
 * degrades to "shared bucket" — not worse — for a spoofed/absent header),
 * but this is a real, open gap, not a solved one: fixing it for real
 * needs {@code proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;}
 * (or, better, a from-scratch {@code X-Real-IP} nginx sets itself) added
 * to nginx.conf/.ci/nginx.frontend.conf — out of scope for this task
 * (frontend/.ci are off-limits here), flagged in tasks.md instead.
 */
@Provider
@ApplicationScoped
public class RateLimitingFilter implements ContainerRequestFilter {

    @Inject
    RateLimiter rateLimiter;

    @ConfigProperty(name = "rate-limit.executions.max-requests", defaultValue = "10")
    int executionsMaxRequests;

    @ConfigProperty(name = "rate-limit.executions.window-seconds", defaultValue = "60")
    int executionsWindowSeconds;

    @ConfigProperty(name = "rate-limit.analysis.max-requests", defaultValue = "30")
    int analysisMaxRequests;

    @ConfigProperty(name = "rate-limit.analysis.window-seconds", defaultValue = "60")
    int analysisWindowSeconds;

    // Quarkus REST (RESTEasy Reactive) resolves @Context fields for
    // Vert.x types per-request even on a singleton/@ApplicationScoped
    // bean like this filter — used only as the fallback when no
    // X-Forwarded-For header is present.
    @Context
    HttpServerRequest vertxRequest;

    @Override
    public void filter(ContainerRequestContext requestContext) {
        if (!"POST".equals(requestContext.getMethod())) {
            return;
        }

        // Quarkus REST's UriInfo.getPath() returns the path WITH a
        // leading slash here (confirmed empirically — differs from the
        // plain-JAX-RS-spec "no leading slash" some other
        // implementations use), so match against "/executions"/
        // "/analysis", not "executions"/"analysis".
        String path = requestContext.getUriInfo().getPath();
        int maxRequests;
        int windowSeconds;
        String bucket;
        if ("/executions".equals(path)) {
            bucket = "executions";
            maxRequests = executionsMaxRequests;
            windowSeconds = executionsWindowSeconds;
        } else if ("/analysis".equals(path)) {
            bucket = "analysis";
            maxRequests = analysisMaxRequests;
            windowSeconds = analysisWindowSeconds;
        } else {
            return;
        }

        String key = bucket + "|" + clientIp(requestContext);
        if (!rateLimiter.tryAcquire(key, maxRequests, windowSeconds)) {
            requestContext.abortWith(Response.status(429)
                    .type(MediaType.APPLICATION_JSON)
                    .entity(new ErrorResponse("rate limit exceeded, try again later"))
                    .build());
        }
    }

    private String clientIp(ContainerRequestContext requestContext) {
        // See the class Javadoc above for why this is trusted despite the
        // current nginx configs not sanitizing/overwriting it.
        String forwardedFor = requestContext.getHeaderString("X-Forwarded-For");
        if (forwardedFor != null && !forwardedFor.isBlank()) {
            // Leftmost entry is the original client in the conventional
            // (comma-separated, closest-hop-appends-last) reading of this
            // header.
            return forwardedFor.split(",")[0].trim();
        }
        if (vertxRequest != null && vertxRequest.remoteAddress() != null) {
            return vertxRequest.remoteAddress().host();
        }
        return "unknown";
    }
}
