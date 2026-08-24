import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { OutputPanelComponent } from './output-panel.component';

describe('OutputPanelComponent', () => {
  function create(output: string, errorMessage: string | null = null) {
    TestBed.configureTestingModule({ imports: [OutputPanelComponent] });
    const fixture = TestBed.createComponent(OutputPanelComponent);
    fixture.componentRef.setInput('output', output);
    fixture.componentRef.setInput('errorMessage', errorMessage);
    fixture.detectChanges();
    return fixture;
  }

  it('shows a placeholder when there is no output yet', () => {
    const fixture = create('');
    expect(fixture.nativeElement.textContent).toContain('sem saída');
  });

  it('renders accumulated stdout text', () => {
    const fixture = create('ola mundo\nsegunda linha\n');
    expect(fixture.nativeElement.querySelector('pre').textContent).toContain('ola mundo');
    expect(fixture.nativeElement.querySelector('pre').textContent).toContain('segunda linha');
  });

  it('renders an error message distinctly, in addition to any stdout already captured', () => {
    const fixture = create('parcial\n', 'algo deu errado');
    expect(fixture.nativeElement.querySelector('pre').textContent).toContain('parcial');
    expect(fixture.nativeElement.querySelector('.error').textContent).toContain('algo deu errado');
  });
});
