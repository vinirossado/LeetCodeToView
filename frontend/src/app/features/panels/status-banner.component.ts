import { Component, computed, input } from '@angular/core';
import { terminalEventMessage, type TerminalEvent } from '../../core/models/execution-event.model';

/**
 * Explains *why* the trace ended whenever a terminal/limit condition is
 * present (timeout, memory_limit_exceeded, output_truncated, stack_overflow,
 * step_limit_exceeded, error) — an explicit product requirement (tasks.md):
 * never leave the UI hanging with no feedback. Shown independent of the
 * step cursor position (TraceStoreService.terminalEvent), since it's
 * information about the whole run, not about whatever step is on screen.
 */
@Component({
  selector: 'app-status-banner',
  standalone: true,
  templateUrl: './status-banner.component.html',
  styleUrl: './status-banner.component.css',
})
export class StatusBannerComponent {
  readonly terminalEvent = input<TerminalEvent | null>(null);

  readonly message = computed(() => {
    const event = this.terminalEvent();
    return event ? terminalEventMessage(event) : null;
  });

  /** step_limit_exceeded is a deliberate scope decision, not a failure — styled as a warning, not an error. */
  readonly isScopeLimit = computed(() => this.terminalEvent()?.type === 'step_limit_exceeded');
}
