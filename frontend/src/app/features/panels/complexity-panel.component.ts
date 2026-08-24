import { Component, computed, input } from '@angular/core';
import type { AnalysisOutcome } from '../../core/models/analysis.model';
import { formatSpaceComplexity, formatTimeComplexity } from '../../core/models/complexity-format';
import type { MethodComplexity } from '../../core/models/complexity.model';

/**
 * Big-O indicator driven by the real `POST /analysis` endpoint
 * (ComplexityApiService). Renders every branch of AnalysisOutcome honestly:
 * ok (including the CLI's own "nenhum método encontrado" wording for an
 * empty result, and always spelling out the Unknown reason text rather than
 * a bare "não determinado" — see spec.md "Limitação conhecida"),
 * unsupported_language (C# has no adapter yet — a permanent, expected
 * response, not an error), and error.
 */
@Component({
  selector: 'app-complexity-panel',
  standalone: true,
  templateUrl: './complexity-panel.component.html',
  styleUrl: './complexity-panel.component.css',
})
export class ComplexityPanelComponent {
  readonly outcome = input<AnalysisOutcome | null>(null);
  readonly loading = input<boolean>(false);

  readonly methods = computed<MethodComplexity[]>(() => {
    const outcome = this.outcome();
    return outcome?.kind === 'ok' ? outcome.methods : [];
  });

  readonly errorMessage = computed(() => {
    const outcome = this.outcome();
    return outcome?.kind === 'error' ? outcome.message : '';
  });

  readonly backendMessage = computed(() => {
    const outcome = this.outcome();
    return outcome?.kind === 'unsupported_language' ? outcome.message : '';
  });

  protected formatTime = formatTimeComplexity;
  protected formatSpace = formatSpaceComplexity;
}
