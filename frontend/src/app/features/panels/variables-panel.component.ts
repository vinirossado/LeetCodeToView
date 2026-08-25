import { Component, computed, input } from '@angular/core';
import type { StepEvent } from '../../core/models/execution-event.model';
import type { Language } from '../../core/models/language';

function formatValue(value: unknown): string {
  if (value === null || value === undefined) return 'null';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

/**
 * Renders locals for the currently viewed step — and, when `frames` is
 * available (JAVA ONLY for now, see execution-event.model.ts's
 * `StepEvent.frames` doc comment), for whichever call-stack frame is
 * currently selected via `selectedFrameIndex` (see app.ts's
 * `selectedFrameIndex` signal and call-stack-panel.component.ts's
 * click-to-inspect), not just always the innermost frame — the
 * Python-Tutor-inspired recursion-clarity item from tasks.md: Guo's own
 * SIGCSE 2013 paper on Python Tutor cites exactly this (per-frame variable
 * inspection while stepping through recursion) as a feature students
 * called "a lifesaver" for the recursive unit.
 *
 * Java always resolves real variable names via JDI. C# resolves them from
 * the Portable PDB emitted by `dotnet build` (see sandbox/src/pdb.rs); when
 * a .pdb is missing or a slot falls outside every known scope, its key
 * falls back to the positional placeholder ("local_0", "local_1", ...) it
 * always used before PDB support existed — this panel shows that honestly,
 * with a visible disclaimer, rather than pretending a fallback key is a
 * real name.
 */
@Component({
  selector: 'app-variables-panel',
  standalone: true,
  templateUrl: './variables-panel.component.html',
  styleUrl: './variables-panel.component.css',
})
export class VariablesPanelComponent {
  readonly language = input.required<Language>();
  readonly currentStep = input<StepEvent | null>(null);
  /** Which call-stack frame's locals to show — 0 (innermost) is the default. */
  readonly selectedFrameIndex = input<number>(0);

  /**
   * The locals actually being shown: the selected frame's, when `frames`
   * data is available and the index is in range — falling back to the
   * step's own top-level `locals` (always the innermost frame) otherwise,
   * which covers both "no frames data at all" (C#/Ruby traces) and "index
   * 0 / out of range" uniformly, since `frames[0].locals` and `locals` are
   * always the same value for a trace that does carry `frames` (Debugger.java
   * populates frames[0] by reusing the same computed innermost locals).
   */
  private readonly activeLocals = computed<Record<string, unknown>>(() => {
    const step = this.currentStep();
    if (!step) return {};
    const frames = step.frames;
    const index = this.selectedFrameIndex();
    if (frames && index > 0 && index < frames.length) {
      return frames[index].locals;
    }
    return step.locals;
  });

  /** The selected frame's method name, for a "viewing: <frame>" label — null when showing the (implicit) innermost frame. */
  readonly selectedFrameName = computed<string | null>(() => {
    const step = this.currentStep();
    const frames = step?.frames;
    const index = this.selectedFrameIndex();
    if (frames && index > 0 && index < frames.length) {
      return frames[index].name;
    }
    return null;
  });

  readonly entries = computed(() => {
    return Object.entries(this.activeLocals()).map(([key, value]) => ({
      key,
      value: formatValue(value),
    }));
  });

  readonly hasPositionalFallback = computed(
    () => this.language() === 'csharp' && this.entries().some((entry) => /^local_\d+$/.test(entry.key)),
  );
}
