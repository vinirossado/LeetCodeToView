import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { PlaybackControlsComponent } from './playback-controls.component';

describe('PlaybackControlsComponent', () => {
  function create(overrides: Partial<Record<'hasStarted' | 'atEnd' | 'totalSteps', unknown>> = {}) {
    TestBed.configureTestingModule({ imports: [PlaybackControlsComponent] });
    const fixture = TestBed.createComponent(PlaybackControlsComponent);
    fixture.componentRef.setInput('hasStarted', overrides['hasStarted'] ?? false);
    fixture.componentRef.setInput('atEnd', overrides['atEnd'] ?? false);
    fixture.componentRef.setInput('totalSteps', overrides['totalSteps'] ?? 5);
    fixture.detectChanges();
    return fixture;
  }

  function button(fixture: ReturnType<typeof create>, title: string): HTMLButtonElement {
    return fixture.nativeElement.querySelector(`button[title="${title}"]`);
  }

  it('emits stepBack/stepForward when the respective buttons are clicked', () => {
    const fixture = create({ hasStarted: true });
    const stepForward = vi.fn();
    const stepBack = vi.fn();
    fixture.componentInstance.stepForward.subscribe(stepForward);
    fixture.componentInstance.stepBack.subscribe(stepBack);

    button(fixture, 'Próximo passo').click();
    button(fixture, 'Passo anterior').click();

    expect(stepForward).toHaveBeenCalledTimes(1);
    expect(stepBack).toHaveBeenCalledTimes(1);
  });

  it('disables "step back" before the trace has started', () => {
    const fixture = create({ hasStarted: false });
    expect(button(fixture, 'Passo anterior').disabled).toBe(true);
  });

  it('disables "step forward" once at the end of the trace', () => {
    const fixture = create({ atEnd: true });
    expect(button(fixture, 'Próximo passo').disabled).toBe(true);
  });

  it('emits jumpToStart/jumpToEnd/runToNextBreakpoint/runToPreviousBreakpoint', () => {
    const fixture = create({ hasStarted: true });
    const jumpToStart = vi.fn();
    const jumpToEnd = vi.fn();
    const runNext = vi.fn();
    const runPrev = vi.fn();
    fixture.componentInstance.jumpToStart.subscribe(jumpToStart);
    fixture.componentInstance.jumpToEnd.subscribe(jumpToEnd);
    fixture.componentInstance.runToNextBreakpoint.subscribe(runNext);
    fixture.componentInstance.runToPreviousBreakpoint.subscribe(runPrev);

    button(fixture, 'Ir para o início').click();
    button(fixture, 'Ir para o fim').click();
    button(fixture, 'Próximo breakpoint').click();
    button(fixture, 'Breakpoint anterior').click();

    expect(jumpToStart).toHaveBeenCalledTimes(1);
    expect(jumpToEnd).toHaveBeenCalledTimes(1);
    expect(runNext).toHaveBeenCalledTimes(1);
    expect(runPrev).toHaveBeenCalledTimes(1);
  });

  it('shows the total step count', () => {
    const fixture = create({ totalSteps: 42 });
    expect(fixture.nativeElement.textContent).toContain('42');
  });
});
