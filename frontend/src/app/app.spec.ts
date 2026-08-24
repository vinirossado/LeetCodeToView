import { TestBed } from '@angular/core/testing';
import { Subject, of } from 'rxjs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
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
