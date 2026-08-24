package com.code2complexity.api.web;

import static org.awaitility.Awaitility.await;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.code2complexity.api.ExecutionStore;
import com.code2complexity.api.model.Execution;
import com.code2complexity.api.model.ExecutionStatus;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import io.quarkus.test.junit.QuarkusTest;
import io.restassured.RestAssured;
import jakarta.inject.Inject;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.WebSocket;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

@QuarkusTest
class ExecutionEventsSocketTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Inject
    ExecutionStore store;

    private static JsonNode step(int line) {
        try {
            return MAPPER.readTree("{\"type\":\"step\",\"line\":" + line + "}");
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    @Test
    @DisplayName("sends already-emitted events, then new ones, in order, until the execution finishes")
    void replaysThenStreamsUntilFinished() throws Exception {
        Execution execution = store.create("java", "int x = 1;");
        store.appendEvent(execution.getId(), step(1));

        List<String> received = new CopyOnWriteArrayList<>();
        CountDownLatch closed = new CountDownLatch(1);

        WebSocket socket = connect("/executions/" + execution.getId() + "/events", received, closed);
        try {
            // Once the replayed event #1 is observed, onOpen has already
            // subscribed (subscription happens strictly before the replay
            // send in ExecutionEventsSocket), so appending #2 now is safe
            // and can't race the subscription.
            await().atMost(Duration.ofSeconds(2)).until(() -> received.size() >= 1);

            store.appendEvent(execution.getId(), step(2));
            store.finish(execution.getId(), ExecutionStatus.COMPLETED);

            assertTrue(closed.await(2, TimeUnit.SECONDS), "socket did not close in time");
        } finally {
            socket.abort();
        }

        List<Integer> lines = received.stream().map(ExecutionEventsSocketTest::lineOf).toList();
        assertEquals(List.of(1, 2), lines);
    }

    @Test
    @DisplayName("closes the connection immediately for an unknown execution_id")
    void closesForUnknownId() throws Exception {
        List<String> received = new CopyOnWriteArrayList<>();
        CountDownLatch closed = new CountDownLatch(1);

        WebSocket socket = connect("/executions/does-not-exist/events", received, closed);
        try {
            assertTrue(closed.await(2, TimeUnit.SECONDS), "socket did not close in time");
        } finally {
            socket.abort();
        }
    }

    private static int lineOf(String json) {
        try {
            return MAPPER.readTree(json).get("line").asInt();
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    private WebSocket connect(String path, List<String> received, CountDownLatch closed) throws Exception {
        URI uri = URI.create("ws://localhost:" + RestAssured.port + path);
        CompletableFuture<WebSocket> future = HttpClient.newHttpClient()
                .newWebSocketBuilder()
                .buildAsync(uri, new WebSocket.Listener() {
                    private final StringBuilder buffer = new StringBuilder();

                    @Override
                    public CompletionStage<?> onText(WebSocket webSocket, CharSequence data, boolean last) {
                        buffer.append(data);
                        webSocket.request(1);
                        if (last) {
                            received.add(buffer.toString());
                            buffer.setLength(0);
                        }
                        return null;
                    }

                    @Override
                    public CompletionStage<?> onClose(WebSocket webSocket, int statusCode, String reason) {
                        closed.countDown();
                        return null;
                    }

                    @Override
                    public void onError(WebSocket webSocket, Throwable error) {
                        closed.countDown();
                    }
                });
        return future.get(5, TimeUnit.SECONDS);
    }
}
