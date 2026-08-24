import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import { CallStackPanelComponent } from './call-stack-panel.component';

describe('CallStackPanelComponent', () => {
  function create(stack: string[] | null) {
    TestBed.configureTestingModule({ imports: [CallStackPanelComponent] });
    const fixture = TestBed.createComponent(CallStackPanelComponent);
    fixture.componentRef.setInput('stack', stack);
    fixture.detectChanges();
    return fixture;
  }

  it('shows a placeholder when there is no stack yet', () => {
    const fixture = create(null);
    expect(fixture.nativeElement.textContent).toContain('nenhuma');
  });

  it('lists stack frames with the top of the stack first', () => {
    const fixture = create(['main', 'helper', 'deepest']);
    const items = fixture.nativeElement.querySelectorAll('li');
    expect(items.length).toBe(3);
    expect(items[0].textContent).toContain('deepest');
    expect(items[2].textContent).toContain('main');
  });
});
