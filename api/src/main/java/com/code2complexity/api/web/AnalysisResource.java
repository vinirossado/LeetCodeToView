package com.code2complexity.api.web;

import com.code2complexity.api.analysis.StaticAnalyzer;
import com.code2complexity.api.error.SandboxErrorSanitizer;
import com.code2complexity.api.metrics.Metrics;
import com.fasterxml.jackson.databind.JsonNode;
import io.quarkus.logging.Log;
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

    // "ruby" added here as a small, separate side-fix while wiring Ruby
    // execution support (tasks.md, Fase 3): static-analyzer/src/ruby_adapter.rs
    // already existed and was already fully validated on its own (see
    // tasks.md's "Adaptador AST (Ruby)" entry), but this endpoint — the
    // only thing the frontend's complexity panel actually calls — never
    // had "ruby" added to either this set or ProcessStaticAnalyzer's own
    // EXTENSIONS map, so POST /analysis with language=ruby would have kept
    // 422ing even after the adapter itself was done. Genuinely a gap left
    // over from that earlier, narrower-scoped task, not something this
    // task's own checklist items (TracePoint runtime) technically required
    // — fixed anyway since leaving it broken would mean a Ruby user could
    // run their code but never see a complexity result for it.
    private static final Set<String> VALID_LANGUAGES = Set.of("java", "csharp", "ruby");

    @Inject
    StaticAnalyzer analyzer;

    @Inject
    Metrics metrics;

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

        // Validation failures above (empty body/bad language/blank code)
        // are deliberately NOT logged/counted here: they're client input
        // errors caught before any real analysis work happens, same
        // category as the 422s ExecutionsResource returns, which aren't
        // tracked either — the metric is meant to answer "how is the
        // static-analyzer subprocess itself doing", not "how many
        // malformed requests arrived".
        try {
            JsonNode result = analyzer.analyze(language, code);
            Log.infof("analysis finished language=%s outcome=success", language);
            metrics.recordAnalysis(language, true);
            return Response.ok(new AnalyzeResponse(result)).build();
        } catch (StaticAnalyzer.UnsupportedLanguageException e) {
            Log.infof("analysis finished language=%s outcome=failure reason=unsupported_language", language);
            metrics.recordAnalysis(language, false);
            return Response.status(501).entity(new ErrorResponse(e.getMessage())).build();
        } catch (Exception e) {
            // static-analyzer never produces a "compiler diagnostic" (it's
            // a parser, not a compiler) — any failure here is internal, so
            // this always sanitizes down to the generic message; see
            // SandboxErrorSanitizer. That call already logs the raw detail
            // server-side (Log.warn) — the line below is deliberately a
            // separate, coarser outcome=failure summary line, consistent
            // with the one logged for the success path above, so grepping
            // "analysis finished" always finds every request regardless of
            // outcome.
            String sanitized = SandboxErrorSanitizer.sanitize(e.getMessage(), "static analysis", e);
            Log.infof("analysis finished language=%s outcome=failure reason=internal_error", language);
            metrics.recordAnalysis(language, false);
            return Response.status(500).entity(new ErrorResponse(sanitized)).build();
        }
    }
}
