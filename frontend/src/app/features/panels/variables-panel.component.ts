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
 * Renders `locals` for the currently viewed step. Java always resolves real
 * variable names via JDI. C# resolves them from the Portable PDB emitted by
 * `dotnet build` (see sandbox/src/pdb.rs); when a .pdb is missing or a slot
 * falls outside every known scope, its key falls back to the positional
 * placeholder ("local_0", "local_1", ...) it always used before PDB
 * support existed — this panel shows that honestly, with a visible
 * disclaimer, rather than pretending a fallback key is a real name.
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

  readonly entries = computed(() => {
    const step = this.currentStep();
    if (!step) return [];
    return Object.entries(step.locals).map(([key, value]) => ({
      key,
      value: formatValue(value),
    }));
  });

  readonly hasPositionalFallback = computed(
    () => this.language() === 'csharp' && this.entries().some((entry) => /^local_\d+$/.test(entry.key)),
  );
}
