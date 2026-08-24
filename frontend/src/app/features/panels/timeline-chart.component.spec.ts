import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import type { StepEvent } from '../../core/models/execution-event.model';
import { TimelineChartComponent } from './timeline-chart.component';

function step(time_ns: number, memory_bytes: number): StepEvent {
  return { type: 'step', line: 1, locals: {}, stack: ['main'], time_ns, memory_bytes };
}

describe('TimelineChartComponent', () => {
  function create(steps: StepEvent[], currentStepIndex = -1) {
    TestBed.configureTestingModule({ imports: [TimelineChartComponent] });
    const fixture = TestBed.createComponent(TimelineChartComponent);
    fixture.componentRef.setInput('steps', steps);
    fixture.componentRef.setInput('currentStepIndex', currentStepIndex);
    fixture.detectChanges();
    return fixture;
  }

  it('always shows the instrumentation-noise disclaimer', () => {
    const fixture = create([]);
    expect(fixture.nativeElement.textContent.toLowerCase()).toContain('instrumentação');
    expect(fixture.nativeElement.textContent.toLowerCase()).toContain('benchmark');
  });

  it('shows a placeholder with no steps yet', () => {
    const fixture = create([]);
    expect(fixture.nativeElement.textContent).toContain('sem dados');
  });

  it('plots one point per step for time_ns and memory_bytes', () => {
    const fixture = create([step(10, 100), step(20, 200), step(15, 150)]);
    const polylines = fixture.nativeElement.querySelectorAll('polyline');
    expect(polylines.length).toBe(2);
    polylines.forEach((line: SVGPolylineElement) => {
      const points = line.getAttribute('points')!.trim().split(/\s+/);
      expect(points.length).toBe(3);
    });
  });
});
