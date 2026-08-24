import { Injectable, inject } from '@angular/core';
import { Observable } from 'rxjs';
import { environment } from '../../../environments/environment';
import type { ExecutionEvent } from '../models/execution-event.model';
import { WEBSOCKET_FACTORY } from './websocket-factory';

/**
 * WebSocket client for `GET /executions/:id/events`. Per spec.md, on connect
 * the server replays every buffered event as individual text frames (one
 * JSON object per frame, not an array) and then streams new events live,
 * closing the socket once the execution reaches a terminal state. This
 * service does no interpretation of that stream beyond parsing frames — the
 * "replay vs. live" distinction and the trace-and-replay navigation model are
 * handled client-side by TraceStoreService, not here.
 */
@Injectable({ providedIn: 'root' })
export class ExecutionSocketService {
  private readonly wsFactory = inject(WEBSOCKET_FACTORY);
  private readonly baseUrl = environment.wsBaseUrl;

  /**
   * Emits one ExecutionEvent per frame, in arrival order. Completes when the
   * server closes the socket (execution finished) and errors on a socket
   * error. Closing the returned subscription closes the underlying socket.
   */
  connect(executionId: string): Observable<ExecutionEvent> {
    return new Observable<ExecutionEvent>((subscriber) => {
      const socket = this.wsFactory(`${this.baseUrl}/executions/${executionId}/events`);

      socket.onmessage = (message: MessageEvent) => {
        try {
          const event = JSON.parse(message.data as string) as ExecutionEvent;
          subscriber.next(event);
        } catch {
          subscriber.error(new Error(`quadro de evento inválido recebido: ${message.data}`));
        }
      };

      socket.onerror = () => {
        subscriber.error(new Error('erro na conexão WebSocket com /executions/:id/events'));
      };

      socket.onclose = () => {
        subscriber.complete();
      };

      return () => socket.close();
    });
  }
}
