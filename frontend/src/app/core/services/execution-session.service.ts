import { HttpErrorResponse } from '@angular/common/http';
import { Injectable, inject, signal } from '@angular/core';
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
  // Deliberately a dedicated signal, NOT derived from `traceStore.status()`.
  // TraceStoreService defaults (and resets) to 'pending' — a value it also
  // uses mid-run for "execution created, not running yet". Deriving isBusy
  // from that status alone meant a completely fresh app (no run ever
  // attempted) read as "busy" from the very first render, permanently
  // disabling the Run button with no run in flight. Found via a Playwright
  // E2E test that clicked Run on a fresh page and discovered the button was
  // ALREADY disabled before the click. This signal instead tracks "a
  // run()/load() was invoked and hasn't reached a terminal outcome yet",
  // starting false and explicitly flipped at every terminal transition
  // below (success, error, and every fallback's own error).
  private readonly busySig = signal<boolean>(false);
  private wsSubscription: Subscription | null = null;

  readonly executionId = this.executionIdSig.asReadonly();
  readonly runError = this.runErrorSig.asReadonly();
  readonly isBusy = this.busySig.asReadonly();

  run(language: Language, code: string): void {
    this.wsSubscription?.unsubscribe();
    this.traceStore.reset();
    this.runErrorSig.set(null);
    this.executionIdSig.set(null);
    this.busySig.set(true);

    this.api.createExecution({ language, code }).subscribe({
      next: ({ execution_id }) => {
        this.executionIdSig.set(execution_id);
        this.traceStore.setStatus('running');
        this.streamLive(execution_id);
      },
      error: (err: unknown) => {
        this.traceStore.setStatus('failed');
        this.busySig.set(false);
        this.runErrorSig.set(extractApiError(err));
      },
    });
  }

  load(executionId: string): void {
    this.wsSubscription?.unsubscribe();
    this.traceStore.reset();
    this.runErrorSig.set(null);
    this.executionIdSig.set(executionId);
    this.busySig.set(true);

    this.api.getTrace(executionId).subscribe({
      next: (trace) => {
        this.traceStore.loadTrace(trace.events);
        this.traceStore.setStatus(trace.status);
        if (trace.status === 'pending' || trace.status === 'running') {
          this.streamLive(executionId, trace.events.length);
        } else {
          this.busySig.set(false);
        }
      },
      error: (err: unknown) => {
        // A stale execution_id from a previous localStorage session (see
        // app.ts's constructor) can point at an execution that no longer
        // exists once the API's in-memory ExecutionStore has been reset
        // (e.g. the container was recreated) — GET /trace 404s here.
        this.traceStore.setStatus('failed');
        this.busySig.set(false);
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
            this.busySig.set(false);
          },
          // Same "never leave busy stuck forever" concern as load()'s error
          // handler — this fallback fetch can itself fail.
          error: (err: unknown) => {
            this.traceStore.setStatus('failed');
            this.busySig.set(false);
            this.runErrorSig.set(extractApiError(err));
          },
        });
      },
      complete: () => {
        const terminal = this.traceStore.terminalEvent();
        this.traceStore.setStatus(terminal?.type === 'error' ? 'failed' : 'completed');
        this.busySig.set(false);
      },
    });
  }
}
