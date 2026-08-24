import { Injectable, computed, signal } from '@angular/core';
import {
  isStdoutEvent,
  isStepEvent,
  isTerminalEvent,
  type ExecutionEvent,
  type StepEvent,
  type TerminalEvent,
} from '../models/execution-event.model';
import type { ExecutionStatus } from '../models/execution.model';

/**
 * Holds the one big ordered array of execution events plus a client-side
 * cursor into it — the central data structure of the whole app (see spec.md
 * "Modelo de execução: trace-and-replay"). There is no live pause/step/
 * continue on the server: the sandbox already ran start-to-finish. All
 * step-forward/back/breakpoint navigation below is pure array indexing, with
 * zero requests to the backend.
 *
 * Cursor semantics: `cursor` ranges over [-1, totalSteps()].
 *   -1              -> before the first step (nothing executed yet)
 *   0..totalSteps-1  -> viewing that step index
 *   totalSteps       -> "fully replayed" pseudo-position, past the last step
 *                        event, which also reveals any trailing non-step
 *                        events (trailing stdout, the terminal event, etc.)
 *
 * "Following live": while a WebSocket stream is still delivering events, the
 * cursor auto-tracks the newest step (see ingestEvent). The moment the user
 * navigates manually away from the live edge (step back, jump to start, a
 * breakpoint run that doesn't land on the tip...), we stop yanking their view
 * forward on every new event. Landing back exactly on the tip (jumpToEnd, or
 * naturally stepping/searching forward into it) re-arms following.
 */
@Injectable({ providedIn: 'root' })
export class TraceStoreService {
  private readonly eventsSig = signal<ExecutionEvent[]>([]);
  private readonly cursorSig = signal<number>(-1);
  private readonly followLiveSig = signal<boolean>(true);
  private readonly breakpointsSig = signal<ReadonlySet<number>>(new Set());
  private readonly statusSig = signal<ExecutionStatus>('pending');

  readonly events = this.eventsSig.asReadonly();
  readonly status = this.statusSig.asReadonly();
  readonly breakpoints = this.breakpointsSig.asReadonly();
  readonly isFollowingLive = this.followLiveSig.asReadonly();

  readonly steps = computed<StepEvent[]>(() => this.eventsSig().filter(isStepEvent));
  readonly totalSteps = computed(() => this.steps().length);

  /** Index (within `events()`) of each step event, in order — used to locate output boundaries. */
  private readonly stepEventIndices = computed(() => {
    const indices: number[] = [];
    this.eventsSig().forEach((event, i) => {
      if (isStepEvent(event)) indices.push(i);
    });
    return indices;
  });

  readonly hasStarted = computed(() => this.cursorSig() > -1);
  readonly atEnd = computed(() => this.cursorSig() >= this.totalSteps());

  readonly currentStepIndex = computed(() => {
    const cursor = this.cursorSig();
    const total = this.totalSteps();
    if (cursor < 0 || total === 0) return -1;
    return Math.min(cursor, total - 1);
  });

  readonly currentStep = computed<StepEvent | null>(() => {
    const idx = this.currentStepIndex();
    return idx >= 0 ? this.steps()[idx] : null;
  });

  /** First terminal/limit event in the trace, if any — surfaced independent of cursor position. */
  readonly terminalEvent = computed<TerminalEvent | null>(
    () => this.eventsSig().find(isTerminalEvent) ?? null,
  );

  /**
   * Stdout accumulated up to (and including) the current step. Output that
   * happens between the current step and the *next* step is considered part
   * of "having reached" the current step (it's a consequence of the line
   * that just ran), so the boundary is just before the next step event —
   * or, past the last step, everything remaining in the trace.
   */
  readonly outputSoFar = computed(() => {
    const cursor = this.cursorSig();
    if (cursor < 0) return '';
    const total = this.totalSteps();
    const events = this.eventsSig();
    const indices = this.stepEventIndices();
    const upTo = cursor + 1 < total ? indices[cursor + 1] - 1 : events.length - 1;
    return events
      .slice(0, upTo + 1)
      .filter(isStdoutEvent)
      .map((e) => e.text)
      .join('\n');
  });

  setStatus(status: ExecutionStatus): void {
    this.statusSig.set(status);
  }

  /** Append one event as it arrives (WebSocket frame). Advances the cursor only while following live. */
  ingestEvent(event: ExecutionEvent): void {
    this.eventsSig.update((events) => [...events, event]);
    if (this.followLiveSig() && isStepEvent(event)) {
      this.cursorSig.set(this.totalSteps());
    }
  }

  /** Replace the whole trace at once (REST fallback / page load after the run finished). Lands fully replayed. */
  loadTrace(events: ExecutionEvent[]): void {
    this.eventsSig.set([...events]);
    this.landOn(this.totalSteps());
  }

  /** Clears the trace and cursor for a new run. Breakpoints are kept — they're a debugging aid across re-runs. */
  reset(): void {
    this.eventsSig.set([]);
    this.cursorSig.set(-1);
    this.followLiveSig.set(true);
    this.statusSig.set('pending');
  }

  /**
   * These two navigate relative to what's currently *displayed*
   * (`currentStepIndex`), not the raw cursor value — the pseudo-end position
   * (cursor === totalSteps) and the last real step index both display the
   * same step (by design, see class doc), so stepping back from pseudo-end
   * must skip past that alias to actually change what's shown.
   */
  stepForward(): void {
    this.landOn(this.currentStepIndex() + 1);
  }

  stepBack(): void {
    this.landOn(this.currentStepIndex() - 1);
  }

  /** Jump directly to a 0-based step index (clamped to the valid range). */
  goToStep(index: number): void {
    this.landOn(index);
  }

  jumpToStart(): void {
    this.landOn(-1);
  }

  jumpToEnd(): void {
    this.landOn(this.totalSteps());
  }

  toggleBreakpoint(line: number): void {
    this.breakpointsSig.update((current) => {
      const next = new Set(current);
      if (next.has(line)) {
        next.delete(line);
      } else {
        next.add(line);
      }
      return next;
    });
  }

  /** Advances to the next step (after the current position) whose line has a breakpoint, or to the end. */
  runToNextBreakpoint(): void {
    const steps = this.steps();
    const bps = this.breakpointsSig();
    for (let i = this.cursorSig() + 1; i < steps.length; i++) {
      if (bps.has(steps[i].line)) {
        this.landOn(i);
        return;
      }
    }
    this.jumpToEnd();
  }

  /** Rewinds to the previous step (before the current position) whose line has a breakpoint, or to the start. */
  runToPreviousBreakpoint(): void {
    const steps = this.steps();
    const bps = this.breakpointsSig();
    for (let i = this.cursorSig() - 1; i >= 0; i--) {
      if (bps.has(steps[i].line)) {
        this.landOn(i);
        return;
      }
    }
    this.jumpToStart();
  }

  private landOn(index: number): void {
    const total = this.totalSteps();
    const clamped = Math.max(-1, Math.min(index, total));
    this.cursorSig.set(clamped);
    this.followLiveSig.set(clamped === total);
  }
}
