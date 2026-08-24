import { TestBed } from '@angular/core/testing';
import { HttpErrorResponse } from '@angular/common/http';
import { Subject, of, throwError } from 'rxjs';
import { beforeEach, describe, expect, it } from 'vitest';
import type { ExecutionEvent, StepEvent } from '../models/execution-event.model';
import type { CreateExecutionResponse, TraceResponse } from '../models/execution.model';
import { ExecutionApiService } from './execution-api.service';
import { ExecutionSessionService } from './execution-session.service';
import { ExecutionSocketService } from './execution-socket.service';
import { TraceStoreService } from './trace-store.service';

function step(line: number): StepEvent {
  return { type: 'step', line, locals: {}, stack: ['main'], time_ns: 1, memory_bytes: 1 };
}

describe('ExecutionSessionService', () => {
  let session: ExecutionSessionService;
  let trace: TraceStoreService;
  let createExecution$: Subject<CreateExecutionResponse>;
  let getTrace$: Subject<TraceResponse>;
  let wsEvents$: Subject<ExecutionEvent>;
  let connectedIds: string[];

  beforeEach(() => {
    createExecution$ = new Subject();
    getTrace$ = new Subject();
    wsEvents$ = new Subject();
    connectedIds = [];

    const fakeApi = {
      createExecution: () => createExecution$.asObservable(),
      getTrace: () => getTrace$.asObservable(),
    };
    const fakeSocket = {
      connect: (id: string) => {
        connectedIds.push(id);
        return wsEvents$.asObservable();
      },
    };

    TestBed.configureTestingModule({
      providers: [
        { provide: ExecutionApiService, useValue: fakeApi },
        { provide: ExecutionSocketService, useValue: fakeSocket },
      ],
    });

    session = TestBed.inject(ExecutionSessionService);
    trace = TestBed.inject(TraceStoreService);
  });

  describe('run()', () => {
    it('POSTs the execution, then connects the WebSocket with the returned id', () => {
      session.run('java', 'int x = 1;');
      createExecution$.next({ execution_id: 'exec-42' });

      expect(session.executionId()).toBe('exec-42');
      expect(connectedIds).toEqual(['exec-42']);
      expect(trace.status()).toBe('running');
    });

    it('ingests events streamed over the socket into the trace store', () => {
      session.run('java', 'int x = 1;');
      createExecution$.next({ execution_id: 'exec-42' });

      wsEvents$.next(step(1));
      wsEvents$.next(step(2));

      expect(trace.totalSteps()).toBe(2);
    });

    it('marks the run failed on a 422 (e.g. invalid language) without touching the trace', () => {
      session.run('java', '');
      createExecution$.error(
        new HttpErrorResponse({ status: 422, error: { error: 'code is required' } }),
      );

      expect(session.runError()).toBe('code is required');
      expect(session.executionId()).toBeNull();
    });

    it('marks status completed when the socket closes with no terminal event', () => {
      session.run('java', 'int x = 1;');
      createExecution$.next({ execution_id: 'exec-42' });
      wsEvents$.next(step(1));
      wsEvents$.complete();

      expect(trace.status()).toBe('completed');
    });

    it('marks status failed when the trace ends with an error event', () => {
      session.run('java', 'int x = 1;');
      createExecution$.next({ execution_id: 'exec-42' });
      wsEvents$.next({ type: 'error', message: 'boom' });
      wsEvents$.complete();

      expect(trace.status()).toBe('failed');
    });
  });

  describe('load() — REST fallback for page reload / reconnect (spec.md "Reconexão")', () => {
    it('hydrates the trace store from GET /trace without touching the WebSocket for an already-finished run', () => {
      session.load('exec-99');
      getTrace$.next({
        execution_id: 'exec-99',
        status: 'completed',
        events: [step(1), step(2)],
      });

      expect(trace.totalSteps()).toBe(2);
      expect(trace.status()).toBe('completed');
      expect(connectedIds).toEqual([]);
    });

    it('reconnects the WebSocket for a still-running execution, skipping the events already replayed via /trace', () => {
      session.load('exec-99');
      getTrace$.next({
        execution_id: 'exec-99',
        status: 'running',
        events: [step(1)],
      });

      expect(connectedIds).toEqual(['exec-99']);

      // Server replays every buffered event on WS connect (spec.md) — the
      // first frame duplicates what GET /trace already gave us.
      wsEvents$.next(step(1));
      expect(trace.totalSteps()).toBe(1);

      wsEvents$.next(step(2));
      expect(trace.totalSteps()).toBe(2);
    });
  });
});
