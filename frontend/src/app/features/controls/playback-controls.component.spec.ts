import { TestBed } from '@angular/core/testing';
import { describe, expect, it, vi } from 'vitest';
import { PlaybackControlsComponent } from './playback-controls.component';

describe('PlaybackControlsComponent', () => {
  function create(
    overrides: Partial<Record<'hasStarted' | 'atEnd' | 'totalSteps' | 'isPlaying', unknown>> = {},
  ) {
    TestBed.configureTestingModule({ imports: [PlaybackControlsComponent] });
    const fixture = TestBed.createComponent(PlaybackControlsComponent);
    fixture.componentRef.setInput('hasStarted', overrides['hasStarted'] ?? false);
    fixture.componentRef.setInput('atEnd', overrides['atEnd'] ?? false);
    fixture.componentRef.setInput('totalSteps', overrides['totalSteps'] ?? 5);
    fixture.componentRef.setInput('isPlaying', overrides['isPlaying'] ?? false);
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

  describe('play/pause toggle button', () => {
    it('shows the "play" title/state when stopped', () => {
      const fixture = create({ isPlaying: false, hasStarted: true });
      expect(button(fixture, 'Reproduzir automaticamente')).toBeTruthy();
      expect(fixture.nativeElement.querySelector('button[title="Pausar reprodução automática"]')).toBeNull();
    });

    it('shows the "pause" title/state when playing', () => {
      const fixture = create({ isPlaying: true, hasStarted: true });
      expect(button(fixture, 'Pausar reprodução automática')).toBeTruthy();
      expect(fixture.nativeElement.querySelector('button[title="Reproduzir automaticamente"]')).toBeNull();
    });

    it('emits togglePlay when clicked', () => {
      const fixture = create({ isPlaying: false });
      const togglePlay = vi.fn();
      fixture.componentInstance.togglePlay.subscribe(togglePlay);

      button(fixture, 'Reproduzir automaticamente').click();

      expect(togglePlay).toHaveBeenCalledTimes(1);
    });

    it('is disabled once at the end of the trace, like "next step"', () => {
      const fixture = create({ atEnd: true, isPlaying: false });
      expect(button(fixture, 'Reproduzir automaticamente').disabled).toBe(true);
    });
  });

  describe('playback speed select', () => {
    function select(fixture: ReturnType<typeof create>): HTMLSelectElement {
      return fixture.nativeElement.querySelector('.speed-select select');
    }

    it('defaults to 1x and lists all six speed options', () => {
      const fixture = create();
      const el = select(fixture);
      expect(el.value).toBe('1');
      const optionValues = Array.from(el.options).map((o) => o.value);
      expect(optionValues).toEqual(['1', '0.75', '0.5', '0.25', '0.1', '0.05']);
    });

    it('reflects the playbackSpeed input', () => {
      TestBed.configureTestingModule({ imports: [PlaybackControlsComponent] });
      const fixture = TestBed.createComponent(PlaybackControlsComponent);
      fixture.componentRef.setInput('playbackSpeed', 0.5);
      fixture.detectChanges();
      expect(select(fixture).value).toBe('0.5');
    });

    it('emits speedChange with the numeric value when changed', () => {
      const fixture = create();
      const speedChange = vi.fn();
      fixture.componentInstance.speedChange.subscribe(speedChange);

      const el = select(fixture);
      el.value = '0.25';
      el.dispatchEvent(new Event('change'));

      expect(speedChange).toHaveBeenCalledWith(0.25);
    });
  });
});
