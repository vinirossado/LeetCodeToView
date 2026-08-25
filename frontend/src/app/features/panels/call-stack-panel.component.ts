import { Component, computed, input } from '@angular/core';

/**
 * Renders the call stack of the currently viewed step. Real bug found and
 * fixed: this component used to `.reverse()` the incoming `stack`, based on
 * a doc comment claiming the backend sends it outermost-first. That's
 * backwards — every driver actually sends innermost-first (the currently
 * executing frame at index 0): confirmed straight from source in
 * `sandbox/jdi/Debugger.java` ("Innermost frames first (frames.get(0) is
 * the current frame..."), mirrored in `sandbox/src/com.rs`'s
 * `get_call_stack_names` (starts at the active frame, walks callers), and
 * in `sandbox/ruby/driver.rb`'s TracePoint-based stack tracking — and
 * verified empirically this session via real traces for all three
 * languages (e.g. Java: `["helper","main"]` while inside `helper`, `helper`
 * first). Reversing an already-innermost-first array pushes the
 * currently-executing frame to the BOTTOM of the rendered list and puts
 * `main`/`<Main>$`/`<main>` on top instead — which is why the panel looked
 * like it "only shows main" for any call stack deeper than 1 frame: the
 * real frame was there, just sorted last. Fixed by rendering the backend's
 * order directly (already innermost-first, which is what a call-stack panel
 * should show on top).
 */
@Component({
  selector: 'app-call-stack-panel',
  standalone: true,
  templateUrl: './call-stack-panel.component.html',
  styleUrl: './call-stack-panel.component.css',
})
export class CallStackPanelComponent {
  readonly stack = input<string[] | null>(null);

  readonly frames = computed(() => this.stack() ?? []);
}
