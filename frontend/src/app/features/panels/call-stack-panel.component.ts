import { Component, computed, input, output } from '@angular/core';
import type { FrameInfo } from '../../core/models/execution-event.model';

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
 *
 * Click-to-inspect (Python-Tutor-inspired recursion-clarity item,
 * tasks.md): when the step's `frames` array is present (JAVA ONLY for
 * now — see execution-event.model.ts's `StepEvent.frames` doc comment),
 * each row becomes a real button; clicking it emits `frameSelect` with
 * that frame's index, and app.ts wires that into a shared
 * `selectedFrameIndex` signal the Variables panel also reads, so a
 * clicked frame's own locals are what gets shown there — instead of
 * always only the innermost frame's, no matter how deep the recursion.
 * For C#/Ruby traces (no `frames`), this falls back to the previous
 * plain, non-interactive list — there is no per-frame data to show yet,
 * so no click affordance is offered rather than one that would silently
 * do nothing.
 */
@Component({
  selector: 'app-call-stack-panel',
  standalone: true,
  templateUrl: './call-stack-panel.component.html',
  styleUrl: './call-stack-panel.component.css',
})
export class CallStackPanelComponent {
  readonly stack = input<string[] | null>(null);
  readonly frames = input<FrameInfo[] | null | undefined>(null);
  readonly selectedFrameIndex = input<number>(0);

  readonly frameSelect = output<number>();

  readonly names = computed(() => this.stack() ?? []);
  /** Whether per-frame locals are available to click through (Java only for now). */
  readonly hasFrameData = computed(() => (this.frames()?.length ?? 0) > 0);

  onSelect(index: number): void {
    if (!this.hasFrameData()) return;
    this.frameSelect.emit(index);
  }
}
