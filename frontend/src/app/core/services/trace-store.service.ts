import { Injectable, OnDestroy, computed, signal } from '@angular/core';
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
 *
 * A breakpoint whose line matches an incoming step while following live
 * stops that auto-follow right there instead of racing on to the tip —
 * this is what makes clicking "Run" with a breakpoint set feel like it
 * actually paused execution, even though the sandbox already ran the whole
 * program start-to-finish and this is just client-side replay catching up
 * to a marked line as events stream in.
 *
 * Autoplay: since the whole trace is already buffered client-side,
 * "playing" it back is nothing more than calling stepForward() on a
 * `setInterval` (see `play`/`pause`/`autoplayTick` below) — no server
 * involvement, same as every other navigation method in this class. Three
 * behavioral decisions were made here (see tasks.md for the write-up):
 *   1. Fixed speed only (AUTOPLAY_INTERVAL_MS), no speed selector yet.
 *   2. Autoplay stops automatically the moment it lands on a breakpointed
 *      line, mirroring what a real debugger's "run" does at a breakpoint —
 *      chosen over "ignore breakpoints and just march through" because this
 *      app already invests in a debugger-like breakpoint feel elsewhere
 *      (landedViaBreakpoint / stoppedAtBreakpoint styling, runToNextBreakpoint).
 *   3. Any manual navigation call (stepBack/stepForward/goToStep/jumpToStart/
 *      jumpToEnd/runToNextBreakpoint/runToPreviousBreakpoint) pauses autoplay first — "touching a control takes
 *      the wheel back" is the least surprising behavior. This is done by
 *      having each of those *public* methods call `pause()` before doing
 *      its own thing; `autoplayTick` itself calls the private `landOn`
 *      directly so it doesn't pause itself on every tick.
 */
@Injectable({ providedIn: 'root' })
export class TraceStoreService implements OnDestroy {
  private readonly eventsSig = signal<ExecutionEvent[]>([]);
  private readonly cursorSig = signal<number>(-1);
  private readonly followLiveSig = signal<boolean>(true);
  private readonly breakpointsSig = signal<ReadonlySet<number>>(new Set());
  private readonly statusSig = signal<ExecutionStatus>('pending');
  /** True only when the cursor's current position was reached by actually matching a breakpoint (see `landOn`). */
  private readonly landedViaBreakpointSig = signal<boolean>(false);
  private readonly isPlayingSig = signal<boolean>(false);
  /** Handle for the autoplay `setInterval`, or null while not playing. */
  private playTimerId: ReturnType<typeof setInterval> | null = null;
  /** Fixed autoplay speed — not user-configurable yet, see class doc / tasks.md. */
  private static readonly AUTOPLAY_INTERVAL_MS = 700;

  readonly events = this.eventsSig.asReadonly();
  readonly status = this.statusSig.asReadonly();
  readonly breakpoints = this.breakpointsSig.asReadonly();
  readonly isFollowingLive = this.followLiveSig.asReadonly();
  readonly landedViaBreakpoint = this.landedViaBreakpointSig.asReadonly();
  readonly isPlaying = this.isPlayingSig.asReadonly();

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

  /**
   * Append one event as it arrives (WebSocket frame). Advances the cursor
   * only while following live — and if this step's line has a breakpoint,
   * lands exactly on it (`landOn` turns following back off, since this
   * step's index isn't the pseudo-end), so later events stop being
   * auto-followed until the user re-arms it (see class doc).
   */
  ingestEvent(event: ExecutionEvent): void {
    this.eventsSig.update((events) => [...events, event]);
    if (this.followLiveSig() && isStepEvent(event)) {
      if (this.breakpointsSig().has(event.line)) {
        this.landOn(this.totalSteps() - 1, true);
      } else {
        this.cursorSig.set(this.totalSteps());
        this.landedViaBreakpointSig.set(false);
      }
    }
  }

  /** Replace the whole trace at once (REST fallback / page load after the run finished). Lands fully replayed. */
  loadTrace(events: ExecutionEvent[]): void {
    this.eventsSig.set([...events]);
    this.landOn(this.totalSteps());
  }

  /** Clears the trace and cursor for a new run. Breakpoints are kept — they're a debugging aid across re-runs. */
  reset(): void {
    this.pause();
    this.eventsSig.set([]);
    this.cursorSig.set(-1);
    this.followLiveSig.set(true);
    this.statusSig.set('pending');
    this.landedViaBreakpointSig.set(false);
  }

  /**
   * These two navigate relative to what's currently *displayed*
   * (`currentStepIndex`), not the raw cursor value — the pseudo-end position
   * (cursor === totalSteps) and the last real step index both display the
   * same step (by design, see class doc), so stepping back from pseudo-end
   * must skip past that alias to actually change what's shown.
   *
   * Each of these public navigation methods pauses autoplay first (see
   * class doc, decision 3) — manual navigation always wins.
   */
  stepForward(): void {
    this.pause();
    this.landOn(this.currentStepIndex() + 1);
  }

  stepBack(): void {
    this.pause();
    this.landOn(this.currentStepIndex() - 1);
  }

  /** Jump directly to a 0-based step index (clamped to the valid range). */
  goToStep(index: number): void {
    this.pause();
    this.landOn(index);
  }

  jumpToStart(): void {
    this.pause();
    this.landOn(-1);
  }

  jumpToEnd(): void {
    this.pause();
    this.landOn(this.totalSteps());
  }

  /**
   * Starts autoplay: steps forward once every AUTOPLAY_INTERVAL_MS until it
   * either reaches the end of the trace or lands on a breakpointed line
   * (see class doc, decisions 1-2). No-op if already playing or if there's
   * nothing left to step into.
   */
  play(): void {
    if (this.isPlayingSig() || this.atEnd() || this.totalSteps() === 0) return;
    this.isPlayingSig.set(true);
    this.playTimerId = setInterval(() => this.autoplayTick(), TraceStoreService.AUTOPLAY_INTERVAL_MS);
  }

  /** Stops autoplay (idempotent — safe to call whether or not it's currently playing). */
  pause(): void {
    this.isPlayingSig.set(false);
    if (this.playTimerId !== null) {
      clearInterval(this.playTimerId);
      this.playTimerId = null;
    }
  }

  togglePlay(): void {
    if (this.isPlayingSig()) {
      this.pause();
    } else {
      this.play();
    }
  }

  /**
   * One autoplay tick. Uses `landOn` directly (not `stepForward`) so it
   * doesn't pause itself on every step — see class doc decision 3.
   */
  private autoplayTick(): void {
    const nextIndex = this.currentStepIndex() + 1;
    if (nextIndex >= this.totalSteps()) {
      // Nothing left to step into — stop cleanly instead of firing
      // uselessly (or erroring) past the end of the trace.
      this.pause();
      this.landOn(this.totalSteps());
      return;
    }
    const hitBreakpoint = this.breakpointsSig().has(this.steps()[nextIndex].line);
    this.landOn(nextIndex, hitBreakpoint);
    if (hitBreakpoint) {
      this.pause();
    }
  }

  ngOnDestroy(): void {
    this.pause();
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

  /**
   * Advances to the next step (after the current position) whose line has a
   * breakpoint, or to the end if none is hit — a breakpoint set on a line
   * with no line-table entry (blank line, comment, a bare `}`) never
   * matches any recorded step, so this silently falls through to the end.
   * `landedViaBreakpoint` distinguishes the two outcomes for the UI: only
   * an actual match marks the landing as a breakpoint hit.
   */
  runToNextBreakpoint(): void {
    this.pause();
    const steps = this.steps();
    const bps = this.breakpointsSig();
    for (let i = this.cursorSig() + 1; i < steps.length; i++) {
      if (bps.has(steps[i].line)) {
        this.landOn(i, true);
        return;
      }
    }
    this.jumpToEnd();
  }

  /** Rewinds to the previous step (before the current position) whose line has a breakpoint, or to the start. */
  runToPreviousBreakpoint(): void {
    this.pause();
    const steps = this.steps();
    const bps = this.breakpointsSig();
    for (let i = this.cursorSig() - 1; i >= 0; i--) {
      if (bps.has(steps[i].line)) {
        this.landOn(i, true);
        return;
      }
    }
    this.jumpToStart();
  }

  private landOn(index: number, viaBreakpoint = false): void {
    const total = this.totalSteps();
    const clamped = Math.max(-1, Math.min(index, total));
    this.cursorSig.set(clamped);
    this.followLiveSig.set(clamped === total);
    this.landedViaBreakpointSig.set(viaBreakpoint);
  }
}
