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

  it('lists stack frames with the currently executing (innermost) frame first, matching the backend order verbatim', () => {
    // Backend contract (see the component's own doc comment): every
    // driver — Debugger.java, com.rs, driver.rb — sends `stack`
    // innermost-first (index 0 = currently executing frame), so the panel
    // must render it as-is, not reversed.
    const fixture = create(['deepest', 'helper', 'main']);
    const items = fixture.nativeElement.querySelectorAll('li');
    expect(items.length).toBe(3);
    expect(items[0].textContent).toContain('deepest');
    expect(items[2].textContent).toContain('main');
  });
});
