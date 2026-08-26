import { Component, input, output } from '@angular/core';

/**
 * Pure presentational controls for navigating the already-buffered trace
 * client-side (spec.md "trace-and-replay"). Emits intent only — all of the
 * actual navigation logic (clamping, breakpoint search, follow-live
 * tracking) lives in TraceStoreService; this component just reflects
 * hasStarted/atEnd to disable the boundary buttons.
 */
@Component({
  selector: 'app-playback-controls',
  standalone: true,
  templateUrl: './playback-controls.component.html',
  styleUrl: './playback-controls.component.css',
})
export class PlaybackControlsComponent {
  /** Allowed speed multipliers — mirrors TraceStoreService.PLAYBACK_SPEEDS (kept as a plain literal here to avoid a service import in a pure presentational component). */
  readonly speedOptions = [1, 0.75, 0.5, 0.25, 0.1, 0.05] as const;

  readonly hasStarted = input<boolean>(false);
  readonly atEnd = input<boolean>(false);
  readonly totalSteps = input<number>(0);
  /** Whether autoplay is currently running — flips the play/pause button's icon and label. */
  readonly isPlaying = input<boolean>(false);
  /** Current autoplay speed multiplier — reflected in the speed <select>. */
  readonly playbackSpeed = input<number>(1);

  readonly stepForward = output<void>();
  readonly stepBack = output<void>();
  readonly jumpToStart = output<void>();
  readonly jumpToEnd = output<void>();
  readonly runToNextBreakpoint = output<void>();
  readonly runToPreviousBreakpoint = output<void>();
  /** Emitted when the play/pause button is clicked — the parent/service owns the actual start/stop logic. */
  readonly togglePlay = output<void>();
  /** Emitted with the newly selected speed multiplier when the speed <select> changes. */
  readonly speedChange = output<number>();

  onSpeedChange(event: Event): void {
    const value = Number((event.target as HTMLSelectElement).value);
    this.speedChange.emit(value);
  }
}
