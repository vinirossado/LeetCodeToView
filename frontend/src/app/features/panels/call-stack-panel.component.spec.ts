import { TestBed } from '@angular/core/testing';
import { describe, expect, it } from 'vitest';
import type { FrameInfo } from '../../core/models/execution-event.model';
import { CallStackPanelComponent } from './call-stack-panel.component';

describe('CallStackPanelComponent', () => {
  function create(stack: string[] | null, frames: FrameInfo[] | null = null) {
    TestBed.configureTestingModule({ imports: [CallStackPanelComponent] });
    const fixture = TestBed.createComponent(CallStackPanelComponent);
    fixture.componentRef.setInput('stack', stack);
    fixture.componentRef.setInput('frames', frames);
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

  describe('click-to-inspect (per-frame locals, Java only for now)', () => {
    function frame(name: string): FrameInfo {
      return { name, locals: {} };
    }

    it('renders frames as clickable buttons when `frames` data is present, and emits frameSelect on click', () => {
      const fixture = create(
        ['helper', 'main'],
        [frame('helper'), frame('main')],
      );
      const buttons = fixture.nativeElement.querySelectorAll('button.frame-btn');
      expect(buttons.length).toBe(2);

      const emitted: number[] = [];
      fixture.componentInstance.frameSelect.subscribe((i: number) => emitted.push(i));
      buttons[1].click();

      expect(emitted).toEqual([1]);
    });

    it('highlights the frame matching selectedFrameIndex', () => {
      const fixture = create(['helper', 'main'], [frame('helper'), frame('main')]);
      fixture.componentRef.setInput('selectedFrameIndex', 1);
      fixture.detectChanges();

      const buttons = fixture.nativeElement.querySelectorAll('button.frame-btn');
      expect(buttons[0].classList.contains('selected')).toBe(false);
      expect(buttons[1].classList.contains('selected')).toBe(true);
    });

    it('falls back to a plain, non-interactive list when `frames` is absent (C#/Ruby traces today)', () => {
      const fixture = create(['helper', 'main'], null);
      expect(fixture.nativeElement.querySelectorAll('button.frame-btn').length).toBe(0);
      expect(fixture.nativeElement.textContent).toContain('helper');
      expect(fixture.nativeElement.textContent).toContain('main');
    });
  });
});
