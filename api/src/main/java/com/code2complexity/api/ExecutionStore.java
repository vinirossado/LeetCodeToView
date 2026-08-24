package com.code2complexity.api;

import com.code2complexity.api.model.Execution;
import com.code2complexity.api.model.ExecutionStatus;
import com.fasterxml.jackson.databind.JsonNode;
import jakarta.enterprise.context.ApplicationScoped;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.UUID;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.LinkedBlockingQueue;

/**
 * In-memory execution state (sufficient for the MVP — see tasks.md,
 * "Fila/estado de execuções em memória (ou Redis) para MVP"). Keeps the
 * full trace per execution id (not just the last N events) so
 * reconnecting via GET /trace never loses anything.
 */
@ApplicationScoped
public class ExecutionStore {

    public sealed interface QueueMessage permits QueueMessage.Event, QueueMessage.Closed {
        record Event(JsonNode json) implements QueueMessage {
        }

        record Closed() implements QueueMessage {
        }
    }

    public record Subscription(List<JsonNode> events, BlockingQueue<QueueMessage> queue) {
    }

    private final Map<String, Execution> executions = new HashMap<>();
    private final Map<String, List<BlockingQueue<QueueMessage>>> subscribers = new HashMap<>();
    private final Object lock = new Object();

    public Execution create(String language, String code) {
        Execution execution = new Execution(UUID.randomUUID().toString(), language, code);
        synchronized (lock) {
            executions.put(execution.getId(), execution);
        }
        return execution;
    }

    public Optional<Execution> find(String id) {
        synchronized (lock) {
            return Optional.ofNullable(executions.get(id));
        }
    }

    public void updateStatus(String id, ExecutionStatus status) {
        synchronized (lock) {
            Execution execution = executions.get(id);
            if (execution != null) {
                execution.setStatus(status);
            }
        }
    }

    public void appendEvent(String id, JsonNode event) {
        synchronized (lock) {
            Execution execution = executions.get(id);
            if (execution == null) {
                return;
            }
            execution.getEvents().add(event);
            List<BlockingQueue<QueueMessage>> queues = subscribers.get(id);
            if (queues != null) {
                for (BlockingQueue<QueueMessage> queue : queues) {
                    queue.add(new QueueMessage.Event(event));
                }
            }
        }
    }

    /**
     * Marks the execution as finished (completed/failed) and closes the
     * subscribed queues — each one gets a {@link QueueMessage.Closed}
     * sentinel so its consumer loop knows to stop.
     */
    public void finish(String id, ExecutionStatus status) {
        synchronized (lock) {
            Execution execution = executions.get(id);
            if (execution == null) {
                return;
            }
            execution.setStatus(status);
            List<BlockingQueue<QueueMessage>> queues = subscribers.remove(id);
            if (queues != null) {
                for (BlockingQueue<QueueMessage> queue : queues) {
                    queue.add(new QueueMessage.Closed());
                }
            }
        }
    }

    /**
     * Atomically returns the trace emitted so far and, if the execution is
     * still running, a queue subscribed to future events. Without this
     * atomicity, an event emitted between "read the trace" and "subscribe"
     * would be lost by the WebSocket client.
     */
    public Optional<Subscription> snapshotAndSubscribe(String id) {
        synchronized (lock) {
            Execution execution = executions.get(id);
            if (execution == null) {
                return Optional.empty();
            }

            List<JsonNode> eventsSnapshot = new ArrayList<>(execution.getEvents());
            boolean stillRunning = execution.getStatus() == ExecutionStatus.PENDING
                    || execution.getStatus() == ExecutionStatus.RUNNING;

            if (stillRunning) {
                BlockingQueue<QueueMessage> queue = new LinkedBlockingQueue<>();
                subscribers.computeIfAbsent(id, key -> new ArrayList<>()).add(queue);
                return Optional.of(new Subscription(eventsSnapshot, queue));
            }
            return Optional.of(new Subscription(eventsSnapshot, null));
        }
    }
}
