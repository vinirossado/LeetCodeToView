package com.code2complexity.api.ratelimit;

import static io.restassured.RestAssured.given;

import com.code2complexity.api.support.FakeSandboxRunner;
import com.code2complexity.api.support.FakeStaticAnalyzer;
import io.quarkus.test.junit.QuarkusTest;
import io.restassured.http.ContentType;
import jakarta.inject.Inject;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

// @QuarkusTest reuses one Quarkus application (and therefore one
// singleton RateLimiter) across every test method in this class, so
// counters are reset between tests explicitly below — otherwise one
// test's requests would bleed into the next's budget.
//
// src/test/resources/application.properties overrides the rate limit
// config down to max-requests=3 for both /executions and /analysis
// (production defaults are 10 and 30) purely so this test doesn't need to
// fire dozens of real requests through the full RestAssured/Quarkus stack
// to reach the limit.
@QuarkusTest
class RateLimitingFilterTest {

    private static final String EXECUTION_BODY =
            "{\"language\":\"java\",\"code\":\"class Main { public static void main(String[] a) { int x = 1; } }\"}";
    private static final String ANALYSIS_BODY = "{\"language\":\"java\",\"code\":\"class Main {}\"}";

    @Inject
    RateLimiter rateLimiter;

    @Inject
    FakeSandboxRunner sandboxRunner;

    @Inject
    FakeStaticAnalyzer staticAnalyzer;

    @BeforeEach
    void resetState() {
        rateLimiter.reset();
        sandboxRunner.reset();
        staticAnalyzer.reset();
    }

    @Test
    @DisplayName("POST /executions: the (max+1)th request from the same IP gets 429")
    void limitsExecutionsPerIp() {
        String ip = "203.0.113.10";

        for (int i = 0; i < 3; i++) {
            postExecution(ip).then().statusCode(201);
        }

        postExecution(ip).then()
                .statusCode(429)
                .body("error", org.hamcrest.Matchers.containsString("rate limit"));
    }

    @Test
    @DisplayName("POST /executions: a different IP has its own, unaffected budget")
    void differentIpsHaveIndependentBudgets() {
        String ipA = "203.0.113.11";
        String ipB = "203.0.113.12";

        for (int i = 0; i < 3; i++) {
            postExecution(ipA).then().statusCode(201);
        }
        // ipA is now exhausted...
        postExecution(ipA).then().statusCode(429);
        // ...but ipB, never having made a request, is unaffected.
        postExecution(ipB).then().statusCode(201);
    }

    @Test
    @DisplayName("POST /analysis: the (max+1)th request from the same IP gets 429, independently of /executions")
    void limitsAnalysisPerIpIndependentlyOfExecutions() {
        String ip = "203.0.113.13";

        // Exhaust /executions for this IP first...
        for (int i = 0; i < 3; i++) {
            postExecution(ip).then().statusCode(201);
        }
        postExecution(ip).then().statusCode(429);

        // ...but /analysis for the SAME IP is a separate bucket, so it's
        // still fully available.
        for (int i = 0; i < 3; i++) {
            postAnalysis(ip).then().statusCode(200);
        }
        postAnalysis(ip).then().statusCode(429);
    }

    private io.restassured.response.Response postExecution(String ip) {
        return given()
                .contentType(ContentType.JSON)
                .header("X-Forwarded-For", ip)
                .body(EXECUTION_BODY)
                .when().post("/executions");
    }

    private io.restassured.response.Response postAnalysis(String ip) {
        return given()
                .contentType(ContentType.JSON)
                .header("X-Forwarded-For", ip)
                .body(ANALYSIS_BODY)
                .when().post("/analysis");
    }
}
