package com.code2complexity.api.web;

import com.code2complexity.api.ExecutionJob;
import com.code2complexity.api.ExecutionStore;
import com.code2complexity.api.model.Execution;
import jakarta.inject.Inject;
import jakarta.ws.rs.Consumes;
import jakarta.ws.rs.GET;
import jakarta.ws.rs.POST;
import jakarta.ws.rs.Path;
import jakarta.ws.rs.PathParam;
import jakarta.ws.rs.Produces;
import jakarta.ws.rs.core.MediaType;
import jakarta.ws.rs.core.Response;
import java.util.Set;

@Path("/executions")
public class ExecutionsResource {

    private static final Set<String> VALID_LANGUAGES = Set.of("java", "csharp");

    @Inject
    ExecutionStore store;

    @Inject
    ExecutionJob job;

    @POST
    @Consumes(MediaType.APPLICATION_JSON)
    @Produces(MediaType.APPLICATION_JSON)
    public Response create(CreateExecutionRequest request) {
        String language = request == null || request.language() == null ? "" : request.language();
        String code = request == null || request.code() == null ? "" : request.code();

        if (!VALID_LANGUAGES.contains(language)) {
            return Response.status(422)
                    .entity(new ErrorResponse("language must be one of: " + String.join(", ", VALID_LANGUAGES)))
                    .build();
        }
        if (code.isBlank()) {
            return Response.status(422).entity(new ErrorResponse("code is required")).build();
        }

        Execution execution = store.create(language, code);
        // Runs on its own virtual thread so the sandbox lifecycle (which
        // blocks until the process exits) never delays the 201 response.
        Thread.ofVirtual().start(() -> job.perform(execution));

        return Response.status(201).entity(new CreateExecutionResponse(execution.getId())).build();
    }

    @GET
    @Path("/{id}/trace")
    @Produces(MediaType.APPLICATION_JSON)
    public Response trace(@PathParam("id") String id) {
        return store.find(id)
                .<Response>map(execution -> Response.ok(new TraceResponse(
                        execution.getId(), execution.getStatus().jsonValue(), execution.getEvents())).build())
                .orElseGet(() -> Response.status(404).entity(new ErrorResponse("execution not found")).build());
    }
}
