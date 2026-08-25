import { HttpErrorResponse } from '@angular/common/http';
import { TestBed } from '@angular/core/testing';
import { Subject, of } from 'rxjs';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from './app';
import type { AnalysisOutcome } from './core/models/analysis.model';
import type { ExecutionEvent, StepEvent } from './core/models/execution-event.model';
import type { CreateExecutionResponse, TraceResponse } from './core/models/execution.model';
import { ComplexityApiService } from './core/services/complexity-api.service';
import { ExecutionApiService } from './core/services/execution-api.service';
import { ExecutionSocketService } from './core/services/execution-socket.service';

// Monaco needs real layout/canvas that jsdom does not provide — the editor
// wrapper itself is unit-tested on its own (code-editor.component.spec.ts),
// so here it's replaced with spies to test App's wiring in isolation.
const editorInstance = {
  setValue: vi.fn(),
  getValue: vi.fn(() => ''),
  onDidChangeModelContent: vi.fn(),
  onMouseDown: vi.fn(),
  deltaDecorations: vi.fn(() => []),
  revealLineInCenter: vi.fn(),
  dispose: vi.fn(),
  getModel: vi.fn(() => ({ getLineCount: () => 10 })),
};
vi.mock('monaco-editor', () => {
  class FakeRange {
    constructor(
      public a?: number,
      public b?: number,
      public c?: number,
      public d?: number,
    ) {}
  }
  return {
    editor: {
      create: vi.fn(() => editorInstance),
      setModelLanguage: vi.fn(),
      MouseTargetType: { GUTTER_GLYPH_MARGIN: 2, GUTTER_LINE_NUMBERS: 3 },
    },
    MouseTargetType: { GUTTER_GLYPH_MARGIN: 2, GUTTER_LINE_NUMBERS: 3 },
    Range: FakeRange,
  };
});

function step(line: number): StepEvent {
  return { type: 'step', line, locals: { x: 1 }, stack: ['main'], time_ns: 1, memory_bytes: 1 };
}

describe('App', () => {
  let createExecution$: Subject<CreateExecutionResponse>;
  let getTrace$: Subject<TraceResponse>;
  let wsEvents$: Subject<ExecutionEvent>;
  let analyzeResult: AnalysisOutcome;

  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();

    createExecution$ = new Subject();
    getTrace$ = new Subject();
    wsEvents$ = new Subject();
    analyzeResult = { kind: 'ok', methods: [] };

    TestBed.configureTestingModule({
      imports: [App],
      providers: [
        {
          provide: ExecutionApiService,
          useValue: {
            createExecution: () => createExecution$.asObservable(),
            getTrace: () => getTrace$.asObservable(),
          },
        },
        { provide: ExecutionSocketService, useValue: { connect: () => wsEvents$.asObservable() } },
        { provide: ComplexityApiService, useValue: { analyze: () => of(analyzeResult) } },
      ],
    });
  });

  afterEach(() => {
    // Several tests below push a ?execution=... query string onto jsdom's
    // location to exercise the shared-link boot path — reset it so it
    // doesn't leak into unrelated tests run afterwards.
    window.history.pushState({}, '', '/');
  });

  function create() {
    const fixture = TestBed.createComponent(App);
    fixture.detectChanges();
    return fixture;
  }

  it('creates the app with Java selected and starter code preloaded', () => {
    const fixture = create();
    expect(fixture.componentInstance.language()).toBe('java');
    expect(fixture.componentInstance.code().length).toBeGreaterThan(0);
  });

  it('Java starter code declares a class literally named Main (required by the real API, 422 otherwise)', () => {
    const fixture = create();
    expect(fixture.componentInstance.code()).toMatch(/\bclass\s+Main\b/);
  });

  it('switching language swaps in that language\'s starter example, replacing whatever was there', () => {
    const fixture = create();
    fixture.componentInstance.code.set('algo que o usuário digitou em java');

    fixture.componentInstance.onLanguageChange({ target: { value: 'csharp' } } as unknown as Event);
    fixture.detectChanges();

    expect(fixture.componentInstance.language()).toBe('csharp');
    expect(fixture.componentInstance.code()).not.toContain('algo que o usuário digitou em java');
    expect(fixture.componentInstance.code().length).toBeGreaterThan(0);
  });

  describe('empty code (UX audit quick win #1)', () => {
    // "code is required" (the API's 422 for blank code — ExecutionsResource
    // `code.isBlank()`) used to be reachable and rendered as a bare,
    // untranslated English string via the generic runError() path. Since
    // onRun() is the ONLY caller that submits user-controlled code (both to
    // POST /executions and to the analysis endpoint), and every starter
    // example is always non-blank, disabling Run whenever the editor is
    // blank makes that error state fully unreachable instead of just
    // prettier — see app.ts's isCodeBlank doc comment.

    it('isCodeBlank is true for empty code and for whitespace-only code, matching the API\'s isBlank() check', () => {
      const fixture = create();
      fixture.componentInstance.code.set('');
      expect(fixture.componentInstance.isCodeBlank()).toBe(true);

      fixture.componentInstance.code.set('   \n\t  ');
      expect(fixture.componentInstance.isCodeBlank()).toBe(true);

      fixture.componentInstance.code.set('int x = 1;');
      expect(fixture.componentInstance.isCodeBlank()).toBe(false);
    });

    it('the Run button is disabled in the DOM when the editor is emptied out', () => {
      const fixture = create();
      fixture.componentInstance.code.set('   ');
      fixture.detectChanges();

      const runBtn = fixture.nativeElement.querySelector('.run-btn') as HTMLButtonElement;
      expect(runBtn.disabled).toBe(true);
    });

    it('the Run button is enabled again once non-blank code is typed back in', () => {
      const fixture = create();
      fixture.componentInstance.code.set('');
      fixture.detectChanges();
      fixture.componentInstance.code.set('int x = 1;');
      fixture.detectChanges();

      const runBtn = fixture.nativeElement.querySelector('.run-btn') as HTMLButtonElement;
      expect(runBtn.disabled).toBe(false);
    });
  });

  it('runs an execution and streams step events into the trace store', () => {
    const fixture = create();
    fixture.componentInstance.onRun();
    createExecution$.next({ execution_id: 'exec-1' });
    wsEvents$.next(step(1));
    fixture.detectChanges();

    expect(fixture.componentInstance.executionId()).toBe('exec-1');
    expect(fixture.componentInstance.totalSteps()).toBe(1);
  });

  it('kicks off a static-analysis request on Run and reflects the outcome', () => {
    analyzeResult = { kind: 'ok', methods: [{ method_name: 'main', line: 1, time: 'Linear', space: 'Constant', evidence: [] }] };
    const fixture = create();
    fixture.componentInstance.onRun();
    fixture.detectChanges();

    expect(fixture.componentInstance.analysisOutcome()).toEqual(analyzeResult);
  });

  it('exposes currentLine/currentStack derived from the current step for the child panels', () => {
    const fixture = create();
    fixture.componentInstance.onRun();
    createExecution$.next({ execution_id: 'exec-1' });
    wsEvents$.next(step(4));
    fixture.detectChanges();

    expect(fixture.componentInstance.currentLine()).toBe(4);
    expect(fixture.componentInstance.currentStack()).toEqual(['main']);
  });

  it('persists the execution id so a reload can reconnect (spec.md "Reconexão")', () => {
    const fixture = create();
    fixture.componentInstance.onRun();
    createExecution$.next({ execution_id: 'exec-persisted' });
    fixture.detectChanges();

    expect(localStorage.getItem('code2complexity.lastExecutionId')).toBe('exec-persisted');
  });

  it('on init, loads a previously persisted execution id via GET /trace', () => {
    localStorage.setItem('code2complexity.lastExecutionId', 'exec-old');
    const fixture = create();
    getTrace$.next({ execution_id: 'exec-old', status: 'completed', events: [step(1), step(2)] });
    fixture.detectChanges();

    expect(fixture.componentInstance.executionId()).toBe('exec-old');
    expect(fixture.componentInstance.totalSteps()).toBe(2);
  });

  it('a stale persisted execution id (404 "execution not found") is cleared from localStorage, so a future reload does not repeat the same error forever', () => {
    localStorage.setItem('code2complexity.lastExecutionId', 'exec-gone');
    const fixture = create();
    getTrace$.error(new HttpErrorResponse({ status: 404, error: { error: 'execution not found' } }));
    fixture.detectChanges();

    expect(fixture.componentInstance.runError()).toContain('execution not found');
    expect(localStorage.getItem('code2complexity.lastExecutionId')).toBeNull();
  });

  it('a transient (non-404) error loading the persisted execution id does NOT clear it, since the execution may still be perfectly valid', () => {
    localStorage.setItem('code2complexity.lastExecutionId', 'exec-transient');
    const fixture = create();
    getTrace$.error(new HttpErrorResponse({ status: 500, error: { error: 'internal server error' } }));
    fixture.detectChanges();

    expect(fixture.componentInstance.runError()).toBeTruthy();
    expect(localStorage.getItem('code2complexity.lastExecutionId')).toBe('exec-transient');
  });

  describe('shared execution links (?execution=<id>)', () => {
    it('on init, a ?execution=<id> query param loads that execution via GET /trace', () => {
      window.history.pushState({}, '', '/?execution=exec-shared');
      const fixture = create();
      getTrace$.next({ execution_id: 'exec-shared', status: 'completed', events: [step(1), step(2), step(3)] });
      fixture.detectChanges();

      expect(fixture.componentInstance.executionId()).toBe('exec-shared');
      expect(fixture.componentInstance.totalSteps()).toBe(3);
    });

    it('a ?execution=<id> query param takes priority over a stored last-execution-id, not a silent fallback to it', () => {
      localStorage.setItem('code2complexity.lastExecutionId', 'exec-own-last');
      window.history.pushState({}, '', '/?execution=exec-shared');
      const fixture = create();

      // Only the shared id's GET /trace should have been requested — the
      // fixture's getTrace$ stub is shared across both ids, so assert via
      // what actually reaches the component instead of call counting.
      getTrace$.next({ execution_id: 'exec-shared', status: 'completed', events: [step(1)] });
      fixture.detectChanges();

      expect(fixture.componentInstance.executionId()).toBe('exec-shared');
    });

    it('loading a shared execution does NOT overwrite the localStorage last-execution-id (session-only visit, see app.ts constructor comment)', () => {
      localStorage.setItem('code2complexity.lastExecutionId', 'exec-own-last');
      window.history.pushState({}, '', '/?execution=exec-shared');
      const fixture = create();
      getTrace$.next({ execution_id: 'exec-shared', status: 'completed', events: [step(1)] });
      fixture.detectChanges();

      expect(fixture.componentInstance.executionId()).toBe('exec-shared');
      expect(localStorage.getItem('code2complexity.lastExecutionId')).toBe('exec-own-last');
    });

    it('a subsequent Run after visiting a shared link persists normally (the suppression is one-shot)', () => {
      window.history.pushState({}, '', '/?execution=exec-shared');
      const fixture = create();
      getTrace$.next({ execution_id: 'exec-shared', status: 'completed', events: [step(1)] });
      fixture.detectChanges();

      fixture.componentInstance.onRun();
      createExecution$.next({ execution_id: 'exec-new-run' });
      fixture.detectChanges();

      expect(localStorage.getItem('code2complexity.lastExecutionId')).toBe('exec-new-run');
    });

    it('a nonexistent shared execution id shows the existing "execution not found" error, same path as a stale localStorage id', () => {
      window.history.pushState({}, '', '/?execution=does-not-exist');
      const fixture = create();
      getTrace$.error(new HttpErrorResponse({ status: 404, error: { error: 'execution not found' } }));
      fixture.detectChanges();

      expect(fixture.componentInstance.runError()).toContain('execution not found');
    });
  });

  describe('share/copy link button', () => {
    function stubClipboard() {
      const writeText = vi.fn().mockResolvedValue(undefined);
      Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
      return writeText;
    }

    it('copies a URL containing ?execution=<current id> to the clipboard', async () => {
      const writeText = stubClipboard();
      const fixture = create();
      fixture.componentInstance.onRun();
      createExecution$.next({ execution_id: 'exec-1' });
      fixture.detectChanges();

      await fixture.componentInstance.onCopyShareLink();

      expect(writeText).toHaveBeenCalledTimes(1);
      const copiedUrl = new URL(writeText.mock.calls[0][0] as string);
      expect(copiedUrl.searchParams.get('execution')).toBe('exec-1');
    });

    it('shows a brief confirmation after a successful copy', async () => {
      stubClipboard();
      const fixture = create();
      fixture.componentInstance.onRun();
      createExecution$.next({ execution_id: 'exec-1' });
      fixture.detectChanges();

      expect(fixture.componentInstance.shareCopied()).toBe(false);
      await fixture.componentInstance.onCopyShareLink();
      expect(fixture.componentInstance.shareCopied()).toBe(true);
    });

    it('does not claim success when the Clipboard API itself rejects', async () => {
      const writeText = vi.fn().mockRejectedValue(new Error('denied'));
      Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
      const fixture = create();
      fixture.componentInstance.onRun();
      createExecution$.next({ execution_id: 'exec-1' });
      fixture.detectChanges();

      await fixture.componentInstance.onCopyShareLink();

      expect(fixture.componentInstance.shareCopied()).toBe(false);
    });

    // UX audit quick win #2: a rejected writeText() (denied permission,
    // insecure/non-HTTPS context) used to leave the button completely
    // unchanged — indistinguishable from the click not having registered at
    // all. shareCopyFailed + shareFallbackUrl give it a visible, distinct
    // failure state instead.
    describe('clipboard write failure (UX audit quick win #2)', () => {
      function stubRejectingClipboard() {
        const writeText = vi.fn().mockRejectedValue(new Error('denied'));
        Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
        return writeText;
      }

      async function runAndStartShare(fixture: ReturnType<typeof create>) {
        fixture.componentInstance.onRun();
        createExecution$.next({ execution_id: 'exec-1' });
        fixture.detectChanges();
      }

      it('sets shareCopyFailed and exposes the raw URL as a fallback when the clipboard write rejects', async () => {
        stubRejectingClipboard();
        const fixture = create();
        await runAndStartShare(fixture);

        expect(fixture.componentInstance.shareCopyFailed()).toBe(false);
        expect(fixture.componentInstance.shareFallbackUrl()).toBeNull();

        await fixture.componentInstance.onCopyShareLink();

        expect(fixture.componentInstance.shareCopyFailed()).toBe(true);
        const fallback = fixture.componentInstance.shareFallbackUrl();
        expect(fallback).not.toBeNull();
        expect(new URL(fallback!).searchParams.get('execution')).toBe('exec-1');
      });

      it('renders a distinct failure label on the button and a selectable fallback input in the DOM', async () => {
        stubRejectingClipboard();
        const fixture = create();
        await runAndStartShare(fixture);

        await fixture.componentInstance.onCopyShareLink();
        fixture.detectChanges();

        const shareBtn = fixture.nativeElement.querySelector('.share-btn') as HTMLButtonElement;
        expect(shareBtn.textContent).toContain('Falha ao copiar');
        expect(shareBtn.classList.contains('copy-failed')).toBe(true);

        const fallbackInput = fixture.nativeElement.querySelector('.share-fallback-input') as HTMLInputElement | null;
        expect(fallbackInput).not.toBeNull();
        expect(fallbackInput!.value).toContain('exec-1');
      });

      it('a subsequent successful copy clears the failure state and the fallback link', async () => {
        const writeText = stubRejectingClipboard();
        const fixture = create();
        await runAndStartShare(fixture);

        await fixture.componentInstance.onCopyShareLink();
        expect(fixture.componentInstance.shareCopyFailed()).toBe(true);

        writeText.mockResolvedValue(undefined);
        await fixture.componentInstance.onCopyShareLink();

        expect(fixture.componentInstance.shareCopyFailed()).toBe(false);
        expect(fixture.componentInstance.shareFallbackUrl()).toBeNull();
        expect(fixture.componentInstance.shareCopied()).toBe(true);
      });
    });

    it('does nothing when there is no execution yet (button is not shown in app.html for this case)', async () => {
      const writeText = stubClipboard();
      const fixture = create();

      await fixture.componentInstance.onCopyShareLink();

      expect(writeText).not.toHaveBeenCalled();
    });
  });

  describe('C# step-through note (UX audit quick win #4: collapsible/dismissible)', () => {
    function switchToCsharp(fixture: ReturnType<typeof create>) {
      fixture.componentInstance.onLanguageChange({ target: { value: 'csharp' } } as unknown as Event);
      fixture.detectChanges();
    }

    it('is expanded (not collapsed) by default when nothing was persisted yet', () => {
      const fixture = create();
      switchToCsharp(fixture);

      expect(fixture.componentInstance.csharpNoteCollapsed()).toBe(false);
      const note = fixture.nativeElement.querySelector('.csharp-note') as HTMLElement;
      expect(note.textContent).toContain('local_N');
    });

    it('toggleCsharpNote collapses it and persists that to localStorage', () => {
      const fixture = create();
      switchToCsharp(fixture);

      fixture.componentInstance.toggleCsharpNote();
      fixture.detectChanges();

      expect(fixture.componentInstance.csharpNoteCollapsed()).toBe(true);
      expect(localStorage.getItem('code2complexity.csharpNoteDismissed')).toBe('true');

      // Full text no longer forced on screen, but the toggle to bring it
      // back is still there.
      const note = fixture.nativeElement.querySelector('.csharp-note') as HTMLElement;
      expect(note.textContent).not.toContain('ICorDebugStepper');
      expect(fixture.nativeElement.querySelector('.csharp-note-toggle')).not.toBeNull();
    });

    it('a dismissal from a previous session stays collapsed on the next load', () => {
      localStorage.setItem('code2complexity.csharpNoteDismissed', 'true');
      const fixture = create();
      switchToCsharp(fixture);

      expect(fixture.componentInstance.csharpNoteCollapsed()).toBe(true);
    });

    it('toggling back open re-persists the expanded state, so the full note is still reachable and stays open on reload', () => {
      localStorage.setItem('code2complexity.csharpNoteDismissed', 'true');
      const fixture = create();
      switchToCsharp(fixture);

      fixture.componentInstance.toggleCsharpNote();

      expect(fixture.componentInstance.csharpNoteCollapsed()).toBe(false);
      expect(localStorage.getItem('code2complexity.csharpNoteDismissed')).toBe('false');
    });
  });

  describe('panel tabs', () => {
    it('defaults to the Variables tab (the one most referenced while stepping)', () => {
      const fixture = create();
      expect(fixture.componentInstance.activeTab()).toBe('variables');
    });

    it('switches the active tab on selection', () => {
      const fixture = create();
      fixture.componentInstance.onTabSelect('timeline');
      fixture.detectChanges();
      expect(fixture.componentInstance.activeTab()).toBe('timeline');
    });
  });

  describe('resizable split', () => {
    function mockLayoutRect(fixture: ReturnType<typeof create>) {
      const layoutEl = fixture.nativeElement.querySelector('.layout') as HTMLElement;
      vi.spyOn(layoutEl, 'getBoundingClientRect').mockReturnValue({
        left: 0,
        width: 1000,
        top: 0,
        height: 0,
        right: 1000,
        bottom: 0,
        x: 0,
        y: 0,
        toJSON: () => {},
      } as DOMRect);
      return layoutEl;
    }

    it('defaults the split ratio when nothing was persisted', () => {
      const fixture = create();
      expect(fixture.componentInstance.splitRatio()).toBeCloseTo(0.62);
    });

    it('restores a previously persisted split ratio', () => {
      localStorage.setItem('code2complexity.splitRatio', '0.4');
      const fixture = create();
      expect(fixture.componentInstance.splitRatio()).toBeCloseTo(0.4);
    });

    it('ignores an out-of-range persisted split ratio and falls back to the default', () => {
      localStorage.setItem('code2complexity.splitRatio', '0.95');
      const fixture = create();
      expect(fixture.componentInstance.splitRatio()).toBeCloseTo(0.62);
    });

    it('clamps the ratio while dragging so neither side can be dragged to zero', () => {
      const fixture = create();
      mockLayoutRect(fixture);

      fixture.componentInstance.onResizerPointerDown({ preventDefault: () => {}, clientX: 620 } as PointerEvent);

      document.dispatchEvent(new MouseEvent('pointermove', { clientX: -5000 }));
      expect(fixture.componentInstance.splitRatio()).toBeCloseTo(0.3);

      document.dispatchEvent(new MouseEvent('pointermove', { clientX: 5000 }));
      expect(fixture.componentInstance.splitRatio()).toBeCloseTo(0.75);

      document.dispatchEvent(new MouseEvent('pointerup'));
    });

    it('persists the split ratio to localStorage once the drag ends', () => {
      const fixture = create();
      mockLayoutRect(fixture);

      fixture.componentInstance.onResizerPointerDown({ preventDefault: () => {}, clientX: 620 } as PointerEvent);
      document.dispatchEvent(new MouseEvent('pointermove', { clientX: 450 }));
      document.dispatchEvent(new MouseEvent('pointerup'));

      expect(localStorage.getItem('code2complexity.splitRatio')).toBe(String(fixture.componentInstance.splitRatio()));
    });
  });
});
