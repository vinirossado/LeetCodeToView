package com.code2complexity.api.web;

import jakarta.ws.rs.GET;
import jakarta.ws.rs.Path;
import jakarta.ws.rs.Produces;
import jakarta.ws.rs.core.MediaType;
import jakarta.ws.rs.core.Response;

// Minimal liveness endpoint, added for the .ci/ production Dockerfile's
// HEALTHCHECK (see tasks.md ".ci/ — deploy pra VPS via Docker Swarm"). This
// is intentionally NOT a real readiness/dependency check (it does not shell
// out to sandbox-runner/static-analyzer to verify they're reachable) — it
// only proves the Quarkus HTTP layer itself is up and answering requests.
// A deeper check (e.g. actually invoking the sandbox-runner/static-analyzer
// binaries) is a reasonable follow-up, not done here to avoid adding
// runtime cost/complexity to every healthcheck tick.
@Path("/health")
public class HealthResource {

    @GET
    @Produces(MediaType.APPLICATION_JSON)
    public Response health() {
        return Response.ok("{\"status\":\"ok\"}").build();
    }
}
