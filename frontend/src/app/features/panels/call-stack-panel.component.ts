import { Component, computed, input } from '@angular/core';

/**
 * Renders the call stack of the currently viewed step. The backend gives
 * `stack` as `["main"]`/outermost-first (see spec.md), so this component
 * reverses it for display — a call-stack panel conventionally shows the
 * innermost (currently executing) frame on top.
 */
@Component({
  selector: 'app-call-stack-panel',
  standalone: true,
  templateUrl: './call-stack-panel.component.html',
  styleUrl: './call-stack-panel.component.css',
})
export class CallStackPanelComponent {
  readonly stack = input<string[] | null>(null);

  readonly frames = computed(() => [...(this.stack() ?? [])].reverse());
}
