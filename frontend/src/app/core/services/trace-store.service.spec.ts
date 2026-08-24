import { beforeEach, describe, expect, it } from 'vitest';
import type { ExecutionEvent, StepEvent } from '../models/execution-event.model';
import { TraceStoreService } from './trace-store.service';

function step(line: number, overrides: Partial<StepEvent> = {}): StepEvent {
  return {
    type: 'step',
    line,
    locals: {},
    stack: ['main'],
    time_ns: 100,
    memory_bytes: 100,
    ...overrides,
  };
}

describe('TraceStoreService', () => {
  let store: TraceStoreService;

  beforeEach(() => {
    store = new TraceStoreService();
  });

  describe('empty state', () => {
    it('starts with no steps and no current step', () => {
      expect(store.totalSteps()).toBe(0);
      expect(store.currentStep()).toBeNull();
      expect(store.hasStarted()).toBe(false);
    });
  });

  describe('ingesting live events (as they arrive over the WebSocket)', () => {
    it('appends events to the buffered trace', () => {
      store.ingestEvent(step(1));
      store.ingestEvent(step(2));
      expect(store.totalSteps()).toBe(2);
    });

    it('follows the live stream by default, keeping the cursor at the newest step', () => {
      store.ingestEvent(step(1));
      expect(store.currentStep()?.line).toBe(1);
      store.ingestEvent(step(2));
      expect(store.currentStep()?.line).toBe(2);
    });

    it('stops following live updates once the user manually steps back', () => {
      store.ingestEvent(step(1));
      store.ingestEvent(step(2));
      store.stepBack();
      expect(store.currentStep()?.line).toBe(1);

      store.ingestEvent(step(3));
      // Manual navigation must not be yanked forward by new incoming events.
      expect(store.currentStep()?.line).toBe(1);
      expect(store.isFollowingLive()).toBe(false);
    });

    it('jumping to the end resumes following live updates', () => {
      store.ingestEvent(step(1));
      store.ingestEvent(step(2));
      store.stepBack();
      store.jumpToEnd();
      expect(store.isFollowingLive()).toBe(true);

      store.ingestEvent(step(3));
      expect(store.currentStep()?.line).toBe(3);
    });

    it('stops following live at the first incoming step whose line has a breakpoint, instead of racing to the tip', () => {
      store.toggleBreakpoint(5);
      store.ingestEvent(step(1));
      store.ingestEvent(step(5)); // hits the breakpoint mid-stream

      expect(store.currentStep()?.line).toBe(5);
      expect(store.landedViaBreakpoint()).toBe(true);
      expect(store.isFollowingLive()).toBe(false);

      // Further live events must not be auto-followed past the stop.
      store.ingestEvent(step(9));
      expect(store.currentStep()?.line).toBe(5);
    });

    it('does not move the step cursor for non-step events (e.g. stdout)', () => {
      store.ingestEvent(step(1));
      store.ingestEvent({ type: 'stdout', text: 'ola' });
      expect(store.currentStep()?.line).toBe(1);
      expect(store.totalSteps()).toBe(1);
    });
  });

  describe('client-side step navigation over an already-loaded trace', () => {
    beforeEach(() => {
      store.ingestEvent(step(1));
      store.ingestEvent(step(2));
      store.ingestEvent(step(3));
      store.jumpToStart();
    });

    it('starts before the first step', () => {
      expect(store.hasStarted()).toBe(false);
      expect(store.currentStep()).toBeNull();
    });

    it('steps forward one line at a time', () => {
      store.stepForward();
      expect(store.currentStep()?.line).toBe(1);
      store.stepForward();
      expect(store.currentStep()?.line).toBe(2);
    });

    it('does not advance past the last step', () => {
      store.jumpToEnd();
      const before = store.currentStep();
      store.stepForward();
      expect(store.currentStep()).toEqual(before);
    });

    it('steps backward one line at a time', () => {
      store.stepForward();
      store.stepForward();
      store.stepBack();
      expect(store.currentStep()?.line).toBe(1);
    });

    it('does not go back past the start', () => {
      store.stepBack();
      expect(store.hasStarted()).toBe(false);
      store.stepBack();
      expect(store.hasStarted()).toBe(false);
    });

    it('jumps directly to an arbitrary step index', () => {
      store.goToStep(2);
      expect(store.currentStep()?.line).toBe(3);
    });

    it('clamps goToStep to the valid range', () => {
      store.goToStep(999);
      expect(store.currentStep()?.line).toBe(3);
      store.goToStep(-999);
      expect(store.hasStarted()).toBe(false);
    });
  });

  describe('breakpoints (purely client-side, no server round-trip)', () => {
    beforeEach(() => {
      store.ingestEvent(step(1));
      store.ingestEvent(step(5));
      store.ingestEvent(step(2));
      store.ingestEvent(step(5));
      store.ingestEvent(step(9));
      store.jumpToStart();
    });

    it('toggles a breakpoint on a line', () => {
      store.toggleBreakpoint(5);
      expect(store.breakpoints().has(5)).toBe(true);
      store.toggleBreakpoint(5);
      expect(store.breakpoints().has(5)).toBe(false);
    });

    it('runs forward to the next step whose line has a breakpoint', () => {
      store.toggleBreakpoint(5);
      store.runToNextBreakpoint();
      expect(store.currentStep()?.line).toBe(5);
      // Running again should land on the *next* occurrence of line 5, not stay put.
      store.runToNextBreakpoint();
      expect(store.currentStep()?.line).toBe(5);
      expect(store.currentStepIndex()).toBe(3);
    });

    it('jumps to the end if no further breakpoint is hit', () => {
      store.toggleBreakpoint(9);
      store.goToStep(4); // already at the only line-9 step
      store.runToNextBreakpoint();
      expect(store.atEnd()).toBe(true);
    });

    it('runs backward to the previous step whose line has a breakpoint', () => {
      store.toggleBreakpoint(2);
      store.jumpToEnd();
      store.runToPreviousBreakpoint();
      expect(store.currentStep()?.line).toBe(2);
    });

    describe('landedViaBreakpoint (drives the red "actually stopped here" decoration)', () => {
      it('is true only when runToNextBreakpoint/runToPreviousBreakpoint actually match a step', () => {
        store.toggleBreakpoint(5);
        store.runToNextBreakpoint();
        expect(store.landedViaBreakpoint()).toBe(true);
      });

      it('is false when no breakpoint is hit and it falls through to the end', () => {
        store.toggleBreakpoint(9);
        store.goToStep(4); // already at the only line-9 step — nothing further to match
        store.runToNextBreakpoint();
        expect(store.landedViaBreakpoint()).toBe(false);
      });

      it('is cleared by any other navigation (step, jump, live ingest)', () => {
        store.toggleBreakpoint(5);
        store.runToNextBreakpoint();
        expect(store.landedViaBreakpoint()).toBe(true);

        store.stepForward();
        expect(store.landedViaBreakpoint()).toBe(false);

        store.runToNextBreakpoint();
        expect(store.landedViaBreakpoint()).toBe(true);
        store.jumpToStart();
        expect(store.landedViaBreakpoint()).toBe(false);
      });
    });
  });

  describe('terminal events', () => {
    it('exposes the terminal event once present in the trace, independent of cursor position', () => {
      store.ingestEvent(step(1));
      store.ingestEvent({ type: 'timeout' });
      store.jumpToStart();

      expect(store.terminalEvent()).toEqual({ type: 'timeout' });
    });

    it('has no terminal event for an ongoing/successful trace', () => {
      store.ingestEvent(step(1));
      expect(store.terminalEvent()).toBeNull();
    });
  });

  describe('output accumulated up to the cursor (respects chronological interleaving with steps)', () => {
    beforeEach(() => {
      // Each stdout event's `text` is one line with no trailing newline —
      // the API strips it via BufferedReader.readLine() (ExecutionJob.java)
      // before wrapping the line as a synthetic event. outputSoFar() is
      // responsible for putting the newlines back when joining lines.
      store.ingestEvent({ type: 'stdout', text: 'antes' });
      store.ingestEvent(step(1));
      store.ingestEvent({ type: 'stdout', text: 'meio' });
      store.ingestEvent(step(2));
      store.ingestEvent({ type: 'stdout', text: 'depois' });
      store.jumpToStart();
    });

    it('shows nothing before stepping starts', () => {
      expect(store.outputSoFar()).toBe('');
    });

    it('shows only stdout that happened at or before the current step', () => {
      store.stepForward();
      expect(store.outputSoFar()).toBe('antes\nmeio');
    });

    it('shows trailing stdout once fully replayed to the end', () => {
      store.jumpToEnd();
      expect(store.outputSoFar()).toBe('antes\nmeio\ndepois');
    });
  });

  describe('loading a full trace at once (REST fallback / page reload after completion)', () => {
    it('replaces the buffered events and starts fully replayed at the end', () => {
      const events: ExecutionEvent[] = [step(1), step(2), { type: 'timeout' }];
      store.loadTrace(events);

      expect(store.totalSteps()).toBe(2);
      expect(store.atEnd()).toBe(true);
      expect(store.currentStep()?.line).toBe(2);
      expect(store.terminalEvent()).toEqual({ type: 'timeout' });
    });
  });

  describe('reset', () => {
    it('clears events and cursor but keeps breakpoints (a debugging aid across re-runs)', () => {
      store.ingestEvent(step(1));
      store.toggleBreakpoint(1);
      store.reset();

      expect(store.totalSteps()).toBe(0);
      expect(store.hasStarted()).toBe(false);
      expect(store.breakpoints().has(1)).toBe(true);
    });
  });
});
