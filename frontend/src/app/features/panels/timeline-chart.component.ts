import { Component, computed, input } from '@angular/core';
import type { StepEvent } from '../../core/models/execution-event.model';

function toPolylinePoints(values: number[]): string {
  if (values.length === 0) return '';
  const max = Math.max(...values, 1);
  const min = Math.min(...values, 0);
  const range = max - min || 1;
  return values
    .map((v, i) => {
      const x = values.length === 1 ? 0 : (i / (values.length - 1)) * 100;
      const y = 40 - ((v - min) / range) * 40;
      return `${x},${y}`;
    })
    .join(' ');
}

/**
 * Timeline of `time_ns`/`memory_bytes` across the buffered steps. These
 * numbers are measured under debugger instrumentation (see spec.md
 * "Precisão das métricas") — the disclaimer here is not optional decoration,
 * it's the explicit product requirement to never present this as a reliable
 * benchmark.
 */
@Component({
  selector: 'app-timeline-chart',
  standalone: true,
  templateUrl: './timeline-chart.component.html',
  styleUrl: './timeline-chart.component.css',
})
export class TimelineChartComponent {
  readonly steps = input<StepEvent[]>([]);
  readonly currentStepIndex = input<number>(-1);

  readonly timePoints = computed(() => toPolylinePoints(this.steps().map((s) => s.time_ns)));
  readonly memoryPoints = computed(() => toPolylinePoints(this.steps().map((s) => s.memory_bytes)));

  readonly markerX = computed<number | null>(() => {
    const n = this.steps().length;
    const idx = this.currentStepIndex();
    if (idx < 0 || n <= 1) return null;
    return (idx / (n - 1)) * 100;
  });
}
