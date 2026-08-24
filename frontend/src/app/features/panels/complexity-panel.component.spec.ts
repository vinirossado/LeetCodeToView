import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import type { AnalysisOutcome } from '../../core/models/analysis.model';
import { ComplexityPanelComponent } from './complexity-panel.component';

describe('ComplexityPanelComponent', () => {
  function create(outcome: AnalysisOutcome | null, loading = false) {
    TestBed.configureTestingModule({ imports: [ComplexityPanelComponent] });
    const fixture = TestBed.createComponent(ComplexityPanelComponent);
    fixture.componentRef.setInput('outcome', outcome);
    fixture.componentRef.setInput('loading', loading);
    fixture.detectChanges();
    return fixture;
  }

  it('shows a loading state while the analysis is running', () => {
    const fixture = create(null, true);
    expect(fixture.nativeElement.textContent.toLowerCase()).toContain('analisando');
  });

  it('shows a neutral placeholder before any analysis has been requested', () => {
    const fixture = create(null, false);
    expect(fixture.nativeElement.textContent.toLowerCase()).toContain('nenhuma análise');
  });

  it('renders Time/Space Big-O for each analyzed method on the ok outcome', () => {
    const fixture = create({
      kind: 'ok',
      methods: [
        {
          method_name: 'main',
          line: 1,
          time: { Polynomial: 2 },
          space: 'Constant',
          evidence: ['linha 3: loop com incremento linear'],
        },
      ],
    });
    const text = fixture.nativeElement.textContent;
    expect(text).toContain('main');
    expect(text).toContain('O(n^2)');
    expect(text).toContain('O(1)');
    expect(text).toContain('linha 3: loop com incremento linear');
  });

  it('shows the CLI-style "nenhum método encontrado" message for an empty ok result', () => {
    const fixture = create({ kind: 'ok', methods: [] });
    expect(fixture.nativeElement.textContent).toContain('nenhum método encontrado');
  });

  it('always renders the Unknown reason text instead of a bare "não determinado"', () => {
    const fixture = create({
      kind: 'ok',
      methods: [
        {
          method_name: 'findIndex',
          line: 2,
          time: { Unknown: 'saída condicional na linha 5' },
          space: 'Constant',
          evidence: [],
        },
      ],
    });
    const text = fixture.nativeElement.textContent;
    expect(text).toContain('não foi possível determinar');
    expect(text).toContain('saída condicional na linha 5');
  });

  it('shows an honest "não suportado para C#" state on unsupported_language, not a generic error', () => {
    const fixture = create({
      kind: 'unsupported_language',
      message: 'static analysis not implemented yet for language: csharp',
    });
    const text = fixture.nativeElement.textContent.toLowerCase();
    expect(text).toContain('c#');
    expect(text).not.toContain('erro');
  });

  it('shows the backend error message on the error outcome', () => {
    const fixture = create({ kind: 'error', message: 'code is required' });
    expect(fixture.nativeElement.textContent).toContain('code is required');
  });
});
