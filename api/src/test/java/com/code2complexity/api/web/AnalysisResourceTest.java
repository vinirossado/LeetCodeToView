package com.code2complexity.api.web;

import static io.restassured.RestAssured.given;
import static org.hamcrest.Matchers.containsString;
import static org.junit.jupiter.api.Assertions.assertEquals;

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

    @BeforeEach
    void resetFake() {
        analyzer.reset();
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
        @DisplayName("returns 501 when the analyzer has no adapter for the language (e.g. C#)")
        void returns501ForUnsupportedLanguage() {
            analyzer.setError(new com.code2complexity.api.analysis.StaticAnalyzer.UnsupportedLanguageException("csharp"));

            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"csharp\",\"code\":\"Console.WriteLine(1);\"}")
                    .when().post("/analysis")
                    .then()
                    .statusCode(501)
                    .body("error", containsString("csharp"));
        }

        @Test
        @DisplayName("returns 500 with the error message when the analyzer fails unexpectedly")
        void returns500OnUnexpectedError() {
            analyzer.setError(new RuntimeException("static-analyzer exited with code 1"));

            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"java\",\"code\":\"class Main {}\"}")
                    .when().post("/analysis")
                    .then()
                    .statusCode(500)
                    .body("error", containsString("static-analyzer exited"));
        }
    }
}
