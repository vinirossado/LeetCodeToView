package com.code2complexity.api.web;

import com.code2complexity.api.ExecutionStore;
import com.code2complexity.api.ExecutionStore.QueueMessage;
import com.code2complexity.api.ExecutionStore.Subscription;
import com.fasterxml.jackson.databind.JsonNode;
import io.quarkus.websockets.next.OnOpen;
import io.quarkus.websockets.next.PathParam;
import io.quarkus.websockets.next.WebSocket;
import io.quarkus.websockets.next.WebSocketConnection;
import io.smallrye.common.annotation.RunOnVirtualThread;
import jakarta.inject.Inject;
import java.util.Optional;
import java.util.concurrent.BlockingQueue;

@WebSocket(path = "/executions/{id}/events")
public class ExecutionEventsSocket {

    @Inject
    ExecutionStore store;

    @Inject
    WebSocketConnection connection;

    // Runs on a virtual thread so blocking on the subscription queue below
    // (queue.take()) is cheap and doesn't tie up a Vert.x event-loop thread.
    @OnOpen
    @RunOnVirtualThread
    public void onOpen(@PathParam("id") String id) throws InterruptedException {
        Optional<Subscription> subscription = store.snapshotAndSubscribe(id);
        if (subscription.isEmpty()) {
            connection.sendTextAndAwait("{\"type\":\"error\",\"message\":\"execution not found\"}");
            connection.closeAndAwait();
            return;
        }

        Subscription sub = subscription.get();
        for (JsonNode event : sub.events()) {
            connection.sendTextAndAwait(event.toString());
        }

        BlockingQueue<QueueMessage> queue = sub.queue();
        if (queue != null) {
            while (true) {
                QueueMessage message = queue.take();
                if (message instanceof QueueMessage.Closed) {
                    break;
                }
                if (message instanceof QueueMessage.Event event) {
                    connection.sendTextAndAwait(event.json().toString());
                }
            }
        }

        connection.closeAndAwait();
    }
}
