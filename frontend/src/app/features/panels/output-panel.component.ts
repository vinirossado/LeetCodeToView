import { Component, input } from '@angular/core';

/**
 * stdout/stderr panel. Per spec.md, there is currently no separate "stderr"
 * JSON event type on the backend — the sandboxed program's real stdout
 * arrives as synthetic `{"type":"stdout"}` events (added by the API layer,
 * see execution-event.model.ts), and the only distinct failure signal is the
 * `{"type":"error","message":"..."}` terminal event. So this single panel is
 * fed by accumulated stdout text plus, when present, that error message —
 * there is no real stderr channel to invent a separate panel for.
 */
@Component({
  selector: 'app-output-panel',
  standalone: true,
  templateUrl: './output-panel.component.html',
  styleUrl: './output-panel.component.css',
})
export class OutputPanelComponent {
  readonly output = input<string>('');
  readonly errorMessage = input<string | null>(null);
}
