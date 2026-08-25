package com.code2complexity.api.web;

import static io.restassured.RestAssured.given;
import static org.awaitility.Awaitility.await;
import static org.hamcrest.Matchers.is;

import com.code2complexity.api.ExecutionStore;
import com.code2complexity.api.metrics.Metrics;
import com.code2complexity.api.model.ExecutionStatus;
import com.code2complexity.api.ratelimit.RateLimiter;
import com.code2complexity.api.support.FakeSandboxRunner;
import com.code2complexity.api.support.FakeStaticAnalyzer;
import io.quarkus.test.junit.QuarkusTest;
import io.restassured.http.ContentType;
import jakarta.inject.Inject;
import java.time.Duration;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;

// @QuarkusTest reuses one Quarkus application (and therefore one singleton
// Metrics bean) across every test method in this class — reset it in
// @BeforeEach, same convention already used for RateLimiter/the fakes in
// the sibling *ResourceTest classes.
@QuarkusTest
class MetricsResourceTest {

    @Inject
    Metrics metrics;

    @Inject
    ExecutionStore store;

    @Inject
    FakeSandboxRunner sandboxRunner;

    @Inject
    FakeStaticAnalyzer staticAnalyzer;

    @Inject
    RateLimiter rateLimiter;

    @BeforeEach
    void resetState() {
        metrics.reset();
        sandboxRunner.reset();
        staticAnalyzer.reset();
        rateLimiter.reset();
    }

    @Nested
    @DisplayName("GET /internal/metrics")
    class Get {

        @Test
        @DisplayName("returns all-zero/empty counters before anything has run")
        void emptyBeforeAnyActivity() {
            given().when().get("/internal/metrics")
                    .then()
                    .statusCode(200)
                    .body("executions_by_language.size()", is(0))
                    .body("executions_by_status.size()", is(0))
                    .body("execution_duration.count", is(0))
                    .body("analysis_by_language.size()", is(0));
        }

        @Test
        @DisplayName("reflects a completed execution's language and status")
        void reflectsCompletedExecution() {
            sandboxRunner.setLines(java.util.List.of("{\"type\":\"step\",\"line\":1}"));

            String executionId = postExecution("java").then().statusCode(201).extract().path("execution_id");
            waitForTerminalStatus(executionId);

            given().when().get("/internal/metrics")
                    .then()
                    .statusCode(200)
                    .body("executions_by_language.java", is(1))
                    .body("executions_by_status.completed", is(1));
        }

        @Test
        @DisplayName("reflects a failed execution's terminal event type")
        void reflectsFailedExecutionTerminalEvent() {
            // No specific terminal event emitted by the (fake) runner, so
            // ExecutionJob appends its own generic {"type":"error",...}
            // fallback — see ExecutionJob#perform's catch block.
            sandboxRunner.setError(new RuntimeException("boom"));

            String executionId = postExecution("java").then().statusCode(201).extract().path("execution_id");
            waitForTerminalStatus(executionId);

            given().when().get("/internal/metrics")
                    .then()
                    .statusCode(200)
                    .body("executions_by_status.failed", is(1))
                    .body("executions_by_terminal_event.error", is(1));
        }

        @Test
        @DisplayName("counts multiple executions across languages independently")
        void countsAcrossLanguages() {
            sandboxRunner.setLines(java.util.List.of("{\"type\":\"step\",\"line\":1}"));

            String id1 = postExecution("java").then().statusCode(201).extract().path("execution_id");
            waitForTerminalStatus(id1);
            String id2 = postExecution("csharp").then().statusCode(201).extract().path("execution_id");
            waitForTerminalStatus(id2);
            String id3 = postExecution("java").then().statusCode(201).extract().path("execution_id");
            waitForTerminalStatus(id3);

            given().when().get("/internal/metrics")
                    .then()
                    .statusCode(200)
                    .body("executions_by_language.java", is(2))
                    .body("executions_by_language.csharp", is(1))
                    .body("executions_by_status.completed", is(3));
        }

        @Test
        @DisplayName("reflects /analysis requests by language and outcome")
        void reflectsAnalysisRequests() {
            staticAnalyzer.setResultJson("[]");
            postAnalysis("java").then().statusCode(200);
            postAnalysis("java").then().statusCode(200);

            staticAnalyzer.setError(new RuntimeException("static-analyzer exploded"));
            postAnalysis("csharp").then().statusCode(500);

            given().when().get("/internal/metrics")
                    .then()
                    .statusCode(200)
                    .body("analysis_by_language.java", is(2))
                    .body("analysis_by_language.csharp", is(1))
                    .body("analysis_by_outcome.success", is(2))
                    .body("analysis_by_outcome.failure", is(1));
        }
    }

    private void waitForTerminalStatus(String id) {
        await().atMost(Duration.ofSeconds(2)).until(() -> {
            ExecutionStatus status = store.find(id).orElseThrow().getStatus();
            return status == ExecutionStatus.COMPLETED || status == ExecutionStatus.FAILED;
        });
    }

    private io.restassured.response.Response postExecution(String language) {
        String code = "csharp".equals(language) ? "Console.WriteLine(1);"
                : "class Main { public static void main(String[] a) { int x = 1; } }";
        return given()
                .contentType(ContentType.JSON)
                .body("{\"language\":\"" + language + "\",\"code\":\"" + code.replace("\"", "\\\"") + "\"}")
                .when().post("/executions");
    }

    private io.restassured.response.Response postAnalysis(String language) {
        String code = "csharp".equals(language) ? "Console.WriteLine(1);" : "class Main {}";
        return given()
                .contentType(ContentType.JSON)
                .body("{\"language\":\"" + language + "\",\"code\":\"" + code.replace("\"", "\\\"") + "\"}")
                .when().post("/analysis");
    }
}
