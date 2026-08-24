package com.code2complexity.api.support;

import com.code2complexity.api.analysis.StaticAnalyzer;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import jakarta.annotation.Priority;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.inject.Alternative;
import java.util.ArrayList;
import java.util.List;

/**
 * Fake analyzer used in tests: never shells out to the real
 * static-analyzer binary. See FakeSandboxRunner for why state here goes
 * through getters/setters rather than public fields (CDI client-proxy
 * field-access pitfall).
 */
@Alternative
@Priority(1)
@ApplicationScoped
public class FakeStaticAnalyzer implements StaticAnalyzer {

    public record AnalyzeCall(String language, String code) {
    }

    private static final ObjectMapper MAPPER = new ObjectMapper();

    private volatile JsonNode result;
    private volatile Exception error;
    private final List<AnalyzeCall> analyzeCalls = new ArrayList<>();

    @Override
    public JsonNode analyze(String language, String code) throws Exception {
        analyzeCalls.add(new AnalyzeCall(language, code));
        if (error != null) {
            throw error;
        }
        return result != null ? result : MAPPER.readTree("[]");
    }

    public void setResultJson(String json) {
        try {
            this.result = MAPPER.readTree(json);
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    public void setError(Exception error) {
        this.error = error;
    }

    public List<AnalyzeCall> getAnalyzeCalls() {
        return analyzeCalls;
    }

    public void reset() {
        result = null;
        error = null;
        analyzeCalls.clear();
    }
}
