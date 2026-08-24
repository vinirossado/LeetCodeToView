package com.code2complexity.api.web;

import static io.restassured.RestAssured.given;
import static org.awaitility.Awaitility.await;
import static org.hamcrest.Matchers.containsString;
import static org.hamcrest.Matchers.equalTo;
import static org.hamcrest.Matchers.is;
import static org.hamcrest.Matchers.matchesPattern;
import static org.junit.jupiter.api.Assertions.assertEquals;

import com.code2complexity.api.ExecutionStore;
import com.code2complexity.api.model.Execution;
import com.code2complexity.api.model.ExecutionStatus;
import com.code2complexity.api.support.FakeSandboxRunner;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import io.quarkus.test.junit.QuarkusTest;
import io.restassured.http.ContentType;
import jakarta.inject.Inject;
import java.time.Duration;
import java.util.List;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;

@QuarkusTest
class ExecutionsResourceTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Inject
    ExecutionStore store;

    @Inject
    FakeSandboxRunner runner;

    @BeforeEach
    void resetFake() {
        runner.reset();
    }

    private static Execution waitForTerminalStatus(ExecutionStore store, String id) {
        await().atMost(Duration.ofSeconds(2)).until(() -> {
            ExecutionStatus status = store.find(id).orElseThrow().getStatus();
            return status == ExecutionStatus.COMPLETED || status == ExecutionStatus.FAILED;
        });
        return store.find(id).orElseThrow();
    }

    private static JsonNode step(int line) {
        try {
            return MAPPER.readTree("{\"type\":\"step\",\"line\":" + line + "}");
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    @Nested
    @DisplayName("POST /executions")
    class CreateExecution {

        @Test
        @DisplayName("creates the execution and returns 201 with a UUID execution_id")
        void createsAndReturns201() {
            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"java\",\"code\":\"class Main { public static void main(String[] a) { int x = 1; } }\"}")
                    .when().post("/executions")
                    .then()
                    .statusCode(201)
                    .body("execution_id", matchesPattern("[0-9a-f-]{36}"));
        }

        @Test
        @DisplayName("accepts 'csharp' as language")
        void acceptsCsharp() {
            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"csharp\",\"code\":\"Console.WriteLine(1);\"}")
                    .when().post("/executions")
                    .then()
                    .statusCode(201);
        }

        @Test
        @DisplayName("rejects an unknown language with 422")
        void rejectsUnknownLanguage() {
            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"python\",\"code\":\"print(1)\"}")
                    .when().post("/executions")
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
                    .when().post("/executions")
                    .then()
                    .statusCode(422)
                    .body("error", containsString("code"));
        }

        @Test
        @DisplayName("rejects Java code without a class named Main, with 422")
        void rejectsJavaWithoutMainClass() {
            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"java\",\"code\":\"class Solution { void run() {} }\"}")
                    .when().post("/executions")
                    .then()
                    .statusCode(422)
                    .body("error", containsString("Main"));
        }

        @Test
        @DisplayName("does not require a Main class for C#, only for Java")
        void doesNotRequireMainClassForCsharp() {
            given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"csharp\",\"code\":\"Console.WriteLine(1);\"}")
                    .when().post("/executions")
                    .then()
                    .statusCode(201);
        }

        @Test
        @DisplayName("rejects a body that isn't valid JSON with 400")
        void rejectsInvalidJson() {
            given()
                    .contentType(ContentType.JSON)
                    .body("{ not json")
                    .when().post("/executions")
                    .then()
                    .statusCode(400);
        }

        @Test
        @DisplayName("invokes the configured runner with the received language and code")
        void invokesRunner() {
            String executionId = given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"java\",\"code\":\"class Main { public static void main(String[] a) { int x = 1; } }\"}")
                    .when().post("/executions")
                    .then().statusCode(201)
                    .extract().path("execution_id");

            waitForTerminalStatus(store, executionId);

            assertEquals(1, runner.getRunCalls().size());
            assertEquals("java", runner.getRunCalls().get(0).language());
            assertEquals("class Main { public static void main(String[] a) { int x = 1; } }", runner.getRunCalls().get(0).code());
        }

        @Test
        @DisplayName("stores the events emitted by the runner and marks the execution completed")
        void storesEventsAndCompletes() {
            runner.setLines(List.of("{\"type\":\"step\",\"line\":1}", "{\"type\":\"step\",\"line\":2}"));

            String executionId = given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"java\",\"code\":\"class Main { public static void main(String[] a) { int x = 1; } }\"}")
                    .when().post("/executions")
                    .then().statusCode(201)
                    .extract().path("execution_id");

            Execution execution = waitForTerminalStatus(store, executionId);

            assertEquals(ExecutionStatus.COMPLETED, execution.getStatus());
            assertEquals(List.of(1, 2), execution.getEvents().stream().map(e -> e.get("line").asInt()).toList());
        }

        @Test
        @DisplayName("wraps non-JSON lines as synthetic stdout events instead of failing the execution")
        void wrapsRawStdoutLines() {
            // sandbox-runner interleaves the sandboxed program's real stdout
            // with its own JSON event lines on the same stream (java.rs/
            // csharp.rs run the target with Stdio::inherit()) — a plain "42"
            // or "ola mundo" line from println/Console.WriteLine is not a
            // parse failure, it's the program talking.
            runner.setLines(List.of("{\"type\":\"step\",\"line\":1}", "ola mundo", "{\"type\":\"step\",\"line\":2}"));

            String executionId = given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"java\",\"code\":\"class Main { public static void main(String[] a) { int x = 1; } }\"}")
                    .when().post("/executions")
                    .then().statusCode(201)
                    .extract().path("execution_id");

            Execution execution = waitForTerminalStatus(store, executionId);

            assertEquals(ExecutionStatus.COMPLETED, execution.getStatus());
            assertEquals(3, execution.getEvents().size());
            assertEquals("step", execution.getEvents().get(0).get("type").asText());
            assertEquals("stdout", execution.getEvents().get(1).get("type").asText());
            assertEquals("ola mundo", execution.getEvents().get(1).get("text").asText());
            assertEquals("step", execution.getEvents().get(2).get("type").asText());
        }

        @Test
        @DisplayName("marks the execution failed when the runner throws")
        void marksFailedOnError() {
            runner.setError(new RuntimeException("nsjail exploded"));

            String executionId = given()
                    .contentType(ContentType.JSON)
                    .body("{\"language\":\"java\",\"code\":\"class Main { public static void main(String[] a) { int x = 1; } }\"}")
                    .when().post("/executions")
                    .then().statusCode(201)
                    .extract().path("execution_id");

            Execution execution = waitForTerminalStatus(store, executionId);

            assertEquals(ExecutionStatus.FAILED, execution.getStatus());
            assertEquals("error", execution.getEvents().get(execution.getEvents().size() - 1).get("type").asText());
        }
    }

    @Nested
    @DisplayName("GET /executions/:id/trace")
    class Trace {

        @Test
        @DisplayName("returns 404 for an unknown id")
        void unknownId() {
            given().when().get("/executions/does-not-exist/trace").then().statusCode(404);
        }

        @Test
        @DisplayName("returns the full trace (status + events) of a finished execution")
        void finishedExecution() {
            Execution execution = store.create("java", "int x = 1;");
            store.appendEvent(execution.getId(), step(1));
            store.finish(execution.getId(), ExecutionStatus.COMPLETED);

            given().when().get("/executions/" + execution.getId() + "/trace")
                    .then()
                    .statusCode(200)
                    .body("execution_id", equalTo(execution.getId()))
                    .body("status", equalTo("completed"))
                    .body("events.size()", is(1))
                    .body("events[0].line", equalTo(1));
        }

        @Test
        @DisplayName("returns a partial trace (status pending) while still running")
        void stillRunning() {
            Execution execution = store.create("java", "int x = 1;");
            store.appendEvent(execution.getId(), step(1));

            given().when().get("/executions/" + execution.getId() + "/trace")
                    .then()
                    .statusCode(200)
                    .body("status", equalTo("pending"))
                    .body("events.size()", is(1));
        }
    }
}
