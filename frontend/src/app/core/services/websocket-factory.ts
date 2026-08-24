import { InjectionToken } from '@angular/core';

/**
 * Minimal surface of the native WebSocket that ExecutionSocketService needs.
 * Extracted as an interface (rather than depending on `WebSocket` directly)
 * so tests can inject a fake socket without touching the real network/jsdom
 * WebSocket implementation.
 */
export interface WebSocketLike {
  onopen: ((event: Event) => void) | null;
  onmessage: ((event: MessageEvent) => void) | null;
  onerror: ((event: Event) => void) | null;
  onclose: ((event: CloseEvent) => void) | null;
  close(): void;
}

export type WebSocketFactory = (url: string) => WebSocketLike;

/** Default factory: construct a real browser WebSocket. */
export const defaultWebSocketFactory: WebSocketFactory = (url) => new WebSocket(url);

export const WEBSOCKET_FACTORY = new InjectionToken<WebSocketFactory>('WEBSOCKET_FACTORY', {
  providedIn: 'root',
  factory: () => defaultWebSocketFactory,
});
