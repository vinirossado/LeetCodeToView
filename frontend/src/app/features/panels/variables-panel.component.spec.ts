import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import type { StepEvent } from '../../core/models/execution-event.model';
import { VariablesPanelComponent } from './variables-panel.component';

function step(locals: Record<string, unknown>): StepEvent {
  return { type: 'step', line: 1, locals, stack: ['main'], time_ns: 1, memory_bytes: 1 };
}

describe('VariablesPanelComponent', () => {
  function create(language: 'java' | 'csharp', currentStep: StepEvent | null) {
    TestBed.configureTestingModule({ imports: [VariablesPanelComponent] });
    const fixture = TestBed.createComponent(VariablesPanelComponent);
    fixture.componentRef.setInput('language', language);
    fixture.componentRef.setInput('currentStep', currentStep);
    fixture.detectChanges();
    return fixture;
  }

  it('shows a placeholder when there is no current step', () => {
    const fixture = create('java', null);
    expect(fixture.nativeElement.textContent).toContain('nenhuma');
  });

  it('lists real variable names and values for Java', () => {
    const fixture = create('java', step({ x: 10, i: 4 }));
    const text = fixture.nativeElement.textContent;
    expect(text).toContain('x');
    expect(text).toContain('10');
    expect(text).toContain('i');
    expect(text).toContain('4');
    expect(text).not.toContain('local_0');
  });

  it('shows the C# positional-placeholder disclaimer next to the variables', () => {
    const fixture = create('csharp', step({ local_0: 'ola mundo' }));
    const text = fixture.nativeElement.textContent;
    expect(text).toContain('local_0');
    expect(text).toContain('ola mundo');
    // Must not present local_0 as if it were the variable's real name.
    expect(text.toLowerCase()).toMatch(/não são os nomes reais|posições|placeholder/);
  });

  it('does not show the C# disclaimer for Java', () => {
    const fixture = create('java', step({ x: 1 }));
    const text = fixture.nativeElement.textContent.toLowerCase();
    expect(text).not.toMatch(/não são os nomes reais/);
  });
});
