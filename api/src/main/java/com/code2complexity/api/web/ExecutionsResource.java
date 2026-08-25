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
import java.util.regex.Pattern;

@Path("/executions")
public class ExecutionsResource {

    private static final Set<String> VALID_LANGUAGES = Set.of("java", "csharp", "ruby");

    // Best-effort check, not a real parser: sandbox-runner always writes
    // Java source to a file named Main.java, and javac requires that name
    // to match a class declared in the file (does NOT have to be public —
    // `class Main { public static void main(...) }` compiles and runs
    // fine without the `public` modifier on the class itself). This only
    // exists to turn a common mistake into an immediate, clear 422 instead
    // of an opaque javac failure surfacing minutes later as `status:
    // failed`; it can have false negatives (unusual formatting) or false
    // positives (a comment/string containing "class Main") — javac itself
    // remains the actual source of truth either way.
    private static final Pattern JAVA_MAIN_CLASS = Pattern.compile("\\bclass\\s+Main\\b");

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
        if ("java".equals(language) && !JAVA_MAIN_CLASS.matcher(code).find()) {
            return Response.status(422)
                    .entity(new ErrorResponse("Java code must declare a class named Main (the file is compiled as Main.java)"))
                    .build();
        }
        // No equivalent naming-convention check for Ruby, same as C# — and
        // for a different reason than C#'s (which has none because
        // top-level statements are its own idiomatic default). Ruby has no
        // required file/class name at all: sandbox-runner's driver.rb does
        // `load` on whatever filename ProcessSandboxRunner writes the
        // source to (writeRubySource writes "main.rb", an arbitrary but
        // fixed name — there is no javac-style "filename must match a
        // public class inside it" constraint the interpreter enforces).

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
                        execution.getId(), execution.getStatus().jsonValue(), execution.getLanguage(),
                        execution.getCode(), execution.getEvents())).build())
                .orElseGet(() -> Response.status(404).entity(new ErrorResponse("execution not found")).build());
    }
}
