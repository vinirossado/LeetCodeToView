package com.code2complexity.api.web;

import com.code2complexity.api.analysis.StaticAnalyzer;
import com.code2complexity.api.error.SandboxErrorSanitizer;
import io.smallrye.common.annotation.Blocking;
import jakarta.inject.Inject;
import jakarta.ws.rs.Consumes;
import jakarta.ws.rs.POST;
import jakarta.ws.rs.Path;
import jakarta.ws.rs.Produces;
import jakarta.ws.rs.core.MediaType;
import jakarta.ws.rs.core.Response;
import java.util.Set;

@Path("/analysis")
public class AnalysisResource {

    private static final Set<String> VALID_LANGUAGES = Set.of("java", "csharp");

    @Inject
    StaticAnalyzer analyzer;

    // Blocking: this shells out to a subprocess and waits for it (see
    // ProcessStaticAnalyzer), so it can't run on Vert.x's event-loop thread.
    @POST
    @Blocking
    @Consumes(MediaType.APPLICATION_JSON)
    @Produces(MediaType.APPLICATION_JSON)
    public Response analyze(AnalyzeRequest request) {
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

        try {
            return Response.ok(new AnalyzeResponse(analyzer.analyze(language, code))).build();
        } catch (StaticAnalyzer.UnsupportedLanguageException e) {
            return Response.status(501).entity(new ErrorResponse(e.getMessage())).build();
        } catch (Exception e) {
            // static-analyzer never produces a "compiler diagnostic" (it's
            // a parser, not a compiler) — any failure here is internal, so
            // this always sanitizes down to the generic message; see
            // SandboxErrorSanitizer.
            String sanitized = SandboxErrorSanitizer.sanitize(e.getMessage(), "static analysis", e);
            return Response.status(500).entity(new ErrorResponse(sanitized)).build();
        }
    }
}
