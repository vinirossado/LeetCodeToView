import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import type { TerminalEvent } from '../../core/models/execution-event.model';
import { StatusBannerComponent } from './status-banner.component';

describe('StatusBannerComponent', () => {
  function create(event: TerminalEvent | null) {
    TestBed.configureTestingModule({ imports: [StatusBannerComponent] });
    const fixture = TestBed.createComponent(StatusBannerComponent);
    fixture.componentRef.setInput('terminalEvent', event);
    fixture.detectChanges();
    return fixture;
  }

  it('renders nothing when there is no terminal event', () => {
    const fixture = create(null);
    expect(fixture.nativeElement.textContent.trim()).toBe('');
  });

  it('explains a timeout instead of leaving the UI hanging', () => {
    const fixture = create({ type: 'timeout' });
    expect(fixture.nativeElement.textContent).toContain('tempo limite');
  });

  it('explains memory_limit_exceeded', () => {
    const fixture = create({ type: 'memory_limit_exceeded' });
    expect(fixture.nativeElement.textContent).toContain('memória');
  });

  it('explains output_truncated', () => {
    const fixture = create({ type: 'output_truncated' });
    expect(fixture.nativeElement.textContent.toLowerCase()).toContain('truncad');
  });

  it('explains stack_overflow', () => {
    const fixture = create({ type: 'stack_overflow' });
    expect(fixture.nativeElement.textContent.toLowerCase()).toContain('pilha');
  });

  it('explains step_limit_exceeded as a deliberate scope limit, not a bug', () => {
    const fixture = create({ type: 'step_limit_exceeded' });
    const text = fixture.nativeElement.textContent.toLowerCase();
    expect(text).toContain('5.000');
    expect(text).toContain('não um bug');
  });

  it('shows the raw error message for a generic error event', () => {
    const fixture = create({ type: 'error', message: 'NullPointerException at Main.java:3' });
    expect(fixture.nativeElement.textContent).toContain('NullPointerException at Main.java:3');
  });
});
