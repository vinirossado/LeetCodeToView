import { TestBed } from '@angular/core/testing';
import { beforeEach, describe, expect, it } from 'vitest';
import type { ExecutionEvent } from '../models/execution-event.model';
import { ExecutionSocketService } from './execution-socket.service';
import { WEBSOCKET_FACTORY, type WebSocketLike } from './websocket-factory';

/** Test double standing in for the native WebSocket — driven manually from the spec. */
class FakeSocket implements WebSocketLike {
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  closed = false;

  constructor(public readonly url: string) {}

  emit(data: unknown): void {
    this.onmessage?.(new MessageEvent('message', { data: JSON.stringify(data) }));
  }

  emitRaw(data: string): void {
    this.onmessage?.(new MessageEvent('message', { data }));
  }

  triggerError(): void {
    this.onerror?.(new Event('error'));
  }

  triggerClose(): void {
    this.onclose?.(new CloseEvent('close'));
  }

  close(): void {
    this.closed = true;
  }
}

describe('ExecutionSocketService', () => {
  let service: ExecutionSocketService;
  let sockets: FakeSocket[];

  beforeEach(() => {
    sockets = [];
    TestBed.configureTestingModule({
      providers: [
        {
          provide: WEBSOCKET_FACTORY,
          useValue: (url: string) => {
            const s = new FakeSocket(url);
            sockets.push(s);
            return s;
          },
        },
      ],
    });
    service = TestBed.inject(ExecutionSocketService);
  });

  it('connects to the events endpoint for the given execution id', () => {
    service.connect('exec-1').subscribe();
    expect(sockets[0].url.endsWith('/executions/exec-1/events')).toBe(true);
  });

  it('builds an absolute ws(s):// URL from location when wsBaseUrl is empty (same-origin deploy)', () => {
    // Regression test: unlike fetch/<a href>, `new WebSocket(url)` does NOT
    // resolve a relative `url` against the page — it throws a synchronous
    // SyntaxError. environment.wsBaseUrl is '' in production on purpose
    // (see environment.ts), so ExecutionSocketService must never hand the
    // browser a bare relative path. This bug shipped once already: the
    // FakeSocket test double above doesn't replicate the real constructor's
    // URL validation, so the previous (relative-URL) test above kept
    // passing while the real app got stuck forever on "Executando…".
    service.connect('exec-1').subscribe();
    const url = sockets[0].url;
    expect(url.startsWith('ws://') || url.startsWith('wss://')).toBe(true);
    expect(url.endsWith('/executions/exec-1/events')).toBe(true);
  });

  it('emits one ExecutionEvent per frame, in arrival order', () => {
    const received: ExecutionEvent[] = [];
    service.connect('exec-1').subscribe((e) => received.push(e));

    sockets[0].emit({ type: 'step', line: 1, locals: {}, stack: ['main'], time_ns: 1, memory_bytes: 1 });
    sockets[0].emit({ type: 'stdout', text: 'ola' });

    expect(received).toEqual([
      { type: 'step', line: 1, locals: {}, stack: ['main'], time_ns: 1, memory_bytes: 1 },
      { type: 'stdout', text: 'ola' },
    ]);
  });

  it('completes the observable when the server closes the socket', () => {
    let completed = false;
    service.connect('exec-1').subscribe({ complete: () => (completed = true) });

    sockets[0].triggerClose();
    expect(completed).toBe(true);
  });

  it('errors the observable on a socket error', () => {
    let error: unknown;
    service.connect('exec-1').subscribe({ error: (e) => (error = e) });

    sockets[0].triggerError();
    expect(error).toBeInstanceOf(Error);
  });

  it('errors on a non-JSON frame instead of silently dropping it', () => {
    let error: unknown;
    service.connect('exec-1').subscribe({ error: (e) => (error = e) });

    sockets[0].emitRaw('not json');
    expect(error).toBeInstanceOf(Error);
  });

  it('closes the underlying socket when the subscription is unsubscribed', () => {
    const sub = service.connect('exec-1').subscribe();
    sub.unsubscribe();
    expect(sockets[0].closed).toBe(true);
  });

  it('surfaces the documented "execution not found" error frame like any other event', () => {
    const received: ExecutionEvent[] = [];
    service.connect('missing-id').subscribe((e) => received.push(e));

    sockets[0].emit({ type: 'error', message: 'execution not found' });
    expect(received).toEqual([{ type: 'error', message: 'execution not found' }]);
  });
});
