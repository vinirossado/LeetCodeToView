package com.code2complexity.api.web;

import static io.restassured.RestAssured.given;
import static org.hamcrest.Matchers.containsString;
import static org.junit.jupiter.api.Assertions.assertEquals;

import com.code2complexity.api.ratelimit.RateLimiter;
import com.code2complexity.api.support.FakeStaticAnalyzer;
import io.quarkus.test.junit.QuarkusTest;
import io.restassured.http.ContentType;
import jakarta.inject.Inject;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;

@QuarkusTest
class AnalysisResourceTest {

    @Inject
    FakeStaticAnalyzer analyzer;

    @Inject
    RateLimiter rateLimiter;

    @BeforeEach
    void resetFake() {
        analyzer.reset();
        // See ExecutionsResourceTest's identical reset for why: one
        // singleton RateLimiter is shared across every @QuarkusTest in
        // this run.
        rateLimiter.reset();
    }

    @Nested
    @DisplayName("POST /analysis")
    class Analyze {

        @Test
        @DisplayName("returns the analyzer's methods array wrapped as { methods: [...] }")
        void returnsMethods() {
            analyzer.setResultJson("""
                    [{"method_name":"main","line":1,"time":"Constant","space":"Constant","evidence":[]}]
                    """);

            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"java\",\"code\":\"class Main {}\"}")
                    .when().post("/analysis")
                    .then()
                    .statusCode(200)
                    .body("methods.size()", org.hamcrest.Matchers.is(1))
                    .body("methods[0].method_name", org.hamcrest.Matchers.equalTo("main"))
                    .body("methods[0].time", org.hamcrest.Matchers.equalTo("Constant"));
        }

        @Test
        @DisplayName("invokes the analyzer with the received language and code")
        void invokesAnalyzer() {
            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"java\",\"code\":\"class Main {}\"}")
                    .when().post("/analysis")
                    .then().statusCode(200);

            assertEquals(1, analyzer.getAnalyzeCalls().size());
            assertEquals("java", analyzer.getAnalyzeCalls().get(0).language());
            assertEquals("class Main {}", analyzer.getAnalyzeCalls().get(0).code());
        }

        @Test
        @DisplayName("rejects an unknown language with 422")
        void rejectsUnknownLanguage() {
            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"python\",\"code\":\"print(1)\"}")
                    .when().post("/analysis")
                    .then()
                    .statusCode(422)
                    .body("error", containsString("language"));
        }

        @Test
        @DisplayName("rejects blank code with 422")
        void rejectsBlankCode() {
            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"java\",\"code\":\"   \"}")
                    .when().post("/analysis")
                    .then()
                    .statusCode(422)
                    .body("error", containsString("code"));
        }

        @Test
        @DisplayName("returns 501 when the analyzer has no adapter for the language")
        void returns501ForUnsupportedLanguage() {
            // csharp now has a real adapter (static-analyzer/src/csharp_adapter.rs),
            // so this exercises the mechanism generically rather than pinning
            // it to a language that would go stale the moment support lands —
            // ProcessStaticAnalyzer's own real language check is covered
            // separately in ProcessStaticAnalyzerTest.
            analyzer.setError(new com.code2complexity.api.analysis.StaticAnalyzer.UnsupportedLanguageException("ruby"));

            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"csharp\",\"code\":\"Console.WriteLine(1);\"}")
                    .when().post("/analysis")
                    .then()
                    .statusCode(501)
                    .body("error", containsString("ruby"));
        }

        @Test
        @DisplayName("returns 500 with a sanitized generic error when the analyzer fails unexpectedly")
        void returns500OnUnexpectedError() {
            // Fase 2 hardening (see SandboxErrorSanitizer): the raw failure
            // text isn't a compiler diagnostic here (static-analyzer is a
            // parser, not a compiler), so it must NOT reach the client
            // verbatim — only the generic message should.
            analyzer.setError(new RuntimeException(
                    "static-analyzer exited with code 1: thread 'main' panicked at src/main.rs:12:5"));

            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"java\",\"code\":\"class Main {}\"}")
                    .when().post("/analysis")
                    .then()
                    .statusCode(500)
                    .body("error", containsString("internal sandbox error"));
        }
    }
}
