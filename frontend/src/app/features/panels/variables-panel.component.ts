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
 * Renders `locals` for the currently viewed step. Handles the documented
 * Java/C# asymmetry (spec.md): Java resolves real variable names via JDI;
 * C# has no PDB parsing yet, so its keys are positional placeholders
 * ("local_0", "local_1", ...) — this panel shows them honestly, with a
 * visible disclaimer, rather than pretending they are real names.
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
}
