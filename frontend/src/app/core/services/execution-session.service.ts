import { HttpErrorResponse } from '@angular/common/http';
import { Injectable, computed, inject, signal } from '@angular/core';
import type { Subscription } from 'rxjs';
import type { ApiErrorResponse } from '../models/execution.model';
import type { Language } from '../models/language';
import { ExecutionApiService } from './execution-api.service';
import { ExecutionSocketService } from './execution-socket.service';
import { TraceStoreService } from './trace-store.service';

function extractApiError(err: unknown): string {
  if (err instanceof HttpErrorResponse) {
    const body = err.error as ApiErrorResponse | undefined;
    if (body?.error) return body.error;
    return `${err.status} ${err.statusText}`.trim();
  }
  return 'Falha inesperada ao comunicar com a API.';
}

/**
 * Coordinates the two entry points into a trace (spec.md "Modelo de
 * execução: trace-and-replay" + "Reconexão") and feeds TraceStoreService,
 * which owns all the client-side navigation state:
 *
 *  - run(): the normal "click Run" path — POST /executions, then stream
 *    /events live over the WebSocket as they're produced.
 *  - load(): the fallback path — GET /trace first (works whether the page
 *    opened after the run finished, or mid-run), and only reconnects the
 *    WebSocket if the run is still going, skipping the replayed events
 *    already covered by the REST snapshot (the server always replays every
 *    buffered event on WS connect).
 */
@Injectable({ providedIn: 'root' })
export class ExecutionSessionService {
  private readonly api = inject(ExecutionApiService);
  private readonly socket = inject(ExecutionSocketService);
  private readonly traceStore = inject(TraceStoreService);

  private readonly executionIdSig = signal<string | null>(null);
  private readonly runErrorSig = signal<string | null>(null);
  private wsSubscription: Subscription | null = null;

  readonly executionId = this.executionIdSig.asReadonly();
  readonly runError = this.runErrorSig.asReadonly();
  readonly isBusy = computed(() => {
    const status = this.traceStore.status();
    return status === 'pending' || status === 'running';
  });

  run(language: Language, code: string): void {
    this.wsSubscription?.unsubscribe();
    this.traceStore.reset();
    this.runErrorSig.set(null);
    this.executionIdSig.set(null);

    this.api.createExecution({ language, code }).subscribe({
      next: ({ execution_id }) => {
        this.executionIdSig.set(execution_id);
        this.traceStore.setStatus('running');
        this.streamLive(execution_id);
      },
      error: (err: unknown) => {
        this.traceStore.setStatus('failed');
        this.runErrorSig.set(extractApiError(err));
      },
    });
  }

  load(executionId: string): void {
    this.wsSubscription?.unsubscribe();
    this.traceStore.reset();
    this.runErrorSig.set(null);
    this.executionIdSig.set(executionId);

    this.api.getTrace(executionId).subscribe({
      next: (trace) => {
        this.traceStore.loadTrace(trace.events);
        this.traceStore.setStatus(trace.status);
        if (trace.status === 'pending' || trace.status === 'running') {
          this.streamLive(executionId, trace.events.length);
        }
      },
      error: (err: unknown) => {
        // `reset()` just above set status to 'pending' — if the trace fetch
        // fails (e.g. a stale execution_id from a previous localStorage
        // session that no longer exists once the API container has been
        // recreated, since ExecutionStore is in-memory only), nothing else
        // would ever move status off 'pending', leaving `isBusy` (and the
        // "Executando…" button) stuck forever even though no run was ever
        // actually attempted this time.
        this.traceStore.setStatus('failed');
        this.runErrorSig.set(extractApiError(err));
      },
    });
  }

  private streamLive(executionId: string, skipReplayCount = 0): void {
    let seen = 0;
    this.wsSubscription = this.socket.connect(executionId).subscribe({
      next: (event) => {
        seen++;
        if (seen <= skipReplayCount) return; // duplicate of what GET /trace already gave us
        this.traceStore.ingestEvent(event);
      },
      error: () => {
        // Connection dropped mid-run — the buffered trace is not lost
        // server-side (spec.md "Reconexão"). One-shot fallback to whatever
        // the API has recorded so far, rather than leaving the UI stuck.
        this.api.getTrace(executionId).subscribe({
          next: (trace) => {
            this.traceStore.loadTrace(trace.events);
            this.traceStore.setStatus(trace.status);
          },
          // Same "never leave status stuck on pending/running" concern as
          // load()'s error handler — this fallback fetch can itself fail.
          error: (err: unknown) => {
            this.traceStore.setStatus('failed');
            this.runErrorSig.set(extractApiError(err));
          },
        });
      },
      complete: () => {
        const terminal = this.traceStore.terminalEvent();
        this.traceStore.setStatus(terminal?.type === 'error' ? 'failed' : 'completed');
      },
    });
  }
}
