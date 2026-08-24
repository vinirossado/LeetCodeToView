// Execution event shapes streamed over the WebSocket / returned by GET /trace.
// Mirrors the backend contract documented in spec.md ("Eventos de execução")
// exactly — field names and casing match the JSON on the wire (snake_case),
// not idiomatic TypeScript, on purpose: this is a wire-format model.

/** One executed line, with the state captured at that point. */
export interface StepEvent {
  type: 'step';
  line: number;
  /**
   * Java: real variable names ("x", "i").
   * C#: positional placeholders ("local_0", "local_1", ...) — no PDB parsing
   * yet, see the "known asymmetry" note in spec.md. Never presented as real
   * names in the UI; callers must check `language` to decide how to label
   * this panel.
   */
  locals: Record<string, unknown>;
  stack: string[];
  /** Wall-clock time, measured under debugger instrumentation — noisy, not a benchmark. */
  time_ns: number;
  /** Memory usage, measured under debugger instrumentation — noisy, not a benchmark. */
  memory_bytes: number;
}

/**
 * Synthetic event added by the API layer (not part of `sandbox/src/events.rs`)
 * because the sandboxed program's real stdout is interleaved on the same
 * stream as the JSON step events. There is currently no separate "stderr"
 * event type on the backend — see the output panel for how this is handled.
 */
export interface StdoutEvent {
  type: 'stdout';
  text: string;
}

export interface TimeoutEvent {
  type: 'timeout';
}

export interface MemoryLimitExceededEvent {
  type: 'memory_limit_exceeded';
}

export interface OutputTruncatedEvent {
  type: 'output_truncated';
}

export interface StackOverflowEvent {
  type: 'stack_overflow';
}

/** Hit the 5,000 step-event cap (see spec.md "Throttling de eventos") — a deliberate scope limit, not a bug. */
export interface StepLimitExceededEvent {
  type: 'step_limit_exceeded';
}

export interface ErrorEvent {
  type: 'error';
  message: string;
}

/** Terminal/limit conditions — each of these ends the trace. */
export type TerminalEvent =
  | TimeoutEvent
  | MemoryLimitExceededEvent
  | OutputTruncatedEvent
  | StackOverflowEvent
  | StepLimitExceededEvent
  | ErrorEvent;

export type ExecutionEvent = StepEvent | StdoutEvent | TerminalEvent;

export type ExecutionEventType = ExecutionEvent['type'];

const TERMINAL_EVENT_TYPES: ReadonlySet<ExecutionEventType> = new Set([
  'timeout',
  'memory_limit_exceeded',
  'output_truncated',
  'stack_overflow',
  'step_limit_exceeded',
  'error',
]);

export function isStepEvent(event: ExecutionEvent): event is StepEvent {
  return event.type === 'step';
}

export function isStdoutEvent(event: ExecutionEvent): event is StdoutEvent {
  return event.type === 'stdout';
}

export function isTerminalEvent(event: ExecutionEvent): event is TerminalEvent {
  return TERMINAL_EVENT_TYPES.has(event.type);
}

/** One human-readable (Portuguese) explanation per terminal event, for the status banner. */
export function terminalEventMessage(event: TerminalEvent): string {
  switch (event.type) {
    case 'timeout':
      return 'A execução ultrapassou o tempo limite e foi interrompida. O trace parcial gerado até esse ponto continua disponível abaixo.';
    case 'memory_limit_exceeded':
      return 'A execução ultrapassou o limite de memória e foi interrompida pelo sandbox.';
    case 'output_truncated':
      return 'A saída (stdout) da execução foi truncada por exceder o limite de tamanho.';
    case 'stack_overflow':
      return 'A execução estourou a pilha de chamadas (stack overflow), provavelmente por recursão sem caso de parada.';
    case 'step_limit_exceeded':
      return 'Essa execução passou de 5.000 passos e o step-through foi interrompido — isso é um limite deliberado de escopo (o visualizador é para entender a mecânica em entradas pequenas/médias, não para medir comportamento em escala), não um bug. Use a análise estática de Big-O para entender o comportamento assintótico.';
    case 'error':
      return `A execução terminou com erro: ${event.message}`;
  }
}
