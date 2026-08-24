package com.code2complexity.api;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.code2complexity.api.model.Execution;
import com.code2complexity.api.model.ExecutionStatus;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.util.List;
import java.util.concurrent.BlockingQueue;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;

// Plain unit test (no @QuarkusTest) — ExecutionStore has no CDI-specific
// behavior beyond being a bean, so a direct `new` is enough and much
// faster to run.
class ExecutionStoreTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    private static JsonNode step(int line) {
        try {
            return MAPPER.readTree("{\"type\":\"step\",\"line\":" + line + "}");
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    @Nested
    @DisplayName("create")
    class Create {

        @Test
        @DisplayName("generates a UUID execution id")
        void generatesUuidId() {
            ExecutionStore store = new ExecutionStore();
            Execution execution = store.create("java", "int x = 1;");

            assertTrue(execution.getId().matches("[0-9a-f-]{36}"));
        }

        @Test
        @DisplayName("stores language and code, starts pending with no events")
        void storesFields() {
            ExecutionStore store = new ExecutionStore();
            Execution execution = store.create("csharp", "Console.WriteLine(1);");

            assertEquals("csharp", execution.getLanguage());
            assertEquals("Console.WriteLine(1);", execution.getCode());
            assertEquals(ExecutionStatus.PENDING, execution.getStatus());
            assertTrue(execution.getEvents().isEmpty());
        }

        @Test
        @DisplayName("generates a different id every time")
        void differentIds() {
            ExecutionStore store = new ExecutionStore();
            Execution a = store.create("java", "1");
            Execution b = store.create("java", "1");

            assertNotEquals(a.getId(), b.getId());
        }
    }

    @Nested
    @DisplayName("find")
    class Find {

        @Test
        @DisplayName("returns empty for an unknown id")
        void unknownId() {
            ExecutionStore store = new ExecutionStore();
            assertTrue(store.find("nope").isEmpty());
        }

        @Test
        @DisplayName("returns the stored execution by id")
        void knownId() {
            ExecutionStore store = new ExecutionStore();
            Execution created = store.create("java", "1");

            assertSame(created, store.find(created.getId()).orElseThrow());
        }
    }

    @Nested
    @DisplayName("appendEvent and finish")
    class AppendAndFinish {

        @Test
        @DisplayName("accumulates events on the execution, in arrival order")
        void accumulatesEvents() {
            ExecutionStore store = new ExecutionStore();
            Execution execution = store.create("java", "1");

            store.appendEvent(execution.getId(), step(1));
            store.appendEvent(execution.getId(), step(2));

            assertEquals(List.of(1, 2), lines(execution));
        }

        @Test
        @DisplayName("is a no-op (doesn't throw) for an unknown id")
        void unknownIdNoop() {
            ExecutionStore store = new ExecutionStore();
            assertDoesNotThrow(() -> store.appendEvent("nope", step(1)));
        }

        @Test
        @DisplayName("updates the execution status on finish")
        void updatesStatus() {
            ExecutionStore store = new ExecutionStore();
            Execution execution = store.create("java", "1");

            store.finish(execution.getId(), ExecutionStatus.COMPLETED);

            assertEquals(ExecutionStatus.COMPLETED, execution.getStatus());
        }
    }

    @Nested
    @DisplayName("snapshotAndSubscribe")
    class SnapshotAndSubscribe {

        @Test
        @DisplayName("returns empty for an unknown id")
        void unknownId() {
            ExecutionStore store = new ExecutionStore();
            assertTrue(store.snapshotAndSubscribe("nope").isEmpty());
        }

        @Test
        @DisplayName("returns the events emitted so far and a queue (execution still running)")
        void runningExecution() {
            ExecutionStore store = new ExecutionStore();
            Execution execution = store.create("java", "1");
            store.appendEvent(execution.getId(), step(1));

            ExecutionStore.Subscription result = store.snapshotAndSubscribe(execution.getId()).orElseThrow();

            assertEquals(List.of(1), lines(result.events()));
            assertNotNull(result.queue());
        }

        @Test
        @DisplayName("receives on the queue the events appended after subscribing")
        void receivesLaterEvents() throws InterruptedException {
            ExecutionStore store = new ExecutionStore();
            Execution execution = store.create("java", "1");

            ExecutionStore.Subscription result = store.snapshotAndSubscribe(execution.getId()).orElseThrow();
            BlockingQueue<ExecutionStore.QueueMessage> queue = result.queue();

            store.appendEvent(execution.getId(), step(1));

            ExecutionStore.QueueMessage message = queue.take();
            ExecutionStore.QueueMessage.Event event = assertInstanceOf(ExecutionStore.QueueMessage.Event.class, message);
            assertEquals(1, event.json().get("line").asInt());
        }

        @Test
        @DisplayName("closes the queue when the execution finishes")
        void closesOnFinish() throws InterruptedException {
            ExecutionStore store = new ExecutionStore();
            Execution execution = store.create("java", "1");

            ExecutionStore.Subscription result = store.snapshotAndSubscribe(execution.getId()).orElseThrow();
            BlockingQueue<ExecutionStore.QueueMessage> queue = result.queue();

            store.finish(execution.getId(), ExecutionStatus.COMPLETED);

            ExecutionStore.QueueMessage message = queue.take();
            assertInstanceOf(ExecutionStore.QueueMessage.Closed.class, message);
        }

        @Test
        @DisplayName("returns a null queue (no subscription) when the execution already finished")
        void alreadyFinished() {
            ExecutionStore store = new ExecutionStore();
            Execution execution = store.create("java", "1");
            store.appendEvent(execution.getId(), step(1));
            store.finish(execution.getId(), ExecutionStatus.COMPLETED);

            ExecutionStore.Subscription result = store.snapshotAndSubscribe(execution.getId()).orElseThrow();

            assertEquals(List.of(1), lines(result.events()));
            assertNull(result.queue());
        }
    }

    private static List<Integer> lines(Execution execution) {
        return lines(execution.getEvents());
    }

    private static List<Integer> lines(List<JsonNode> events) {
        return events.stream().map(event -> event.get("line").asInt()).toList();
    }
}
