import { TestBed } from '@angular/core/testing';
import { beforeEach, describe, expect, it, vi } from 'vitest';

// Monaco needs real layout/canvas/workers that jsdom does not provide, and we
// only want to verify *our* wiring (create/update calls, decorations,
// gutter-click -> breakpoint toggle), not Monaco's own rendering — so the
// whole module is replaced with spies.
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
      public startLineNumber: number,
      public startColumn: number,
      public endLineNumber: number,
      public endColumn: number,
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

import { CodeEditorComponent } from './code-editor.component';

describe('CodeEditorComponent', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    TestBed.configureTestingModule({ imports: [CodeEditorComponent] });
  });

  function create() {
    const fixture = TestBed.createComponent(CodeEditorComponent);
    fixture.componentRef.setInput('language', 'java');
    fixture.componentRef.setInput('value', 'int x = 1;');
    fixture.detectChanges();
    return fixture;
  }

  it('creates a Monaco editor on init with the given initial value and language', async () => {
    const monaco = await import('monaco-editor');
    create();
    expect(monaco.editor.create).toHaveBeenCalledTimes(1);
    const [, options] = (monaco.editor.create as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(options.value).toBe('int x = 1;');
    expect(options.language).toBe('java');
  });

  it('maps the "csharp" language input to Monaco\'s "csharp" language id', async () => {
    const monaco = await import('monaco-editor');
    const fixture = TestBed.createComponent(CodeEditorComponent);
    fixture.componentRef.setInput('language', 'csharp');
    fixture.componentRef.setInput('value', '');
    fixture.detectChanges();
    const [, options] = (monaco.editor.create as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(options.language).toBe('csharp');
  });

  it('re-applies a line-highlight decoration when currentLine changes', () => {
    const fixture = create();
    fixture.componentRef.setInput('currentLine', 4);
    fixture.detectChanges();
    expect(editorInstance.deltaDecorations).toHaveBeenCalled();
    expect(editorInstance.revealLineInCenter).toHaveBeenCalledWith(4);
  });

  it('clears the highlight when currentLine is null', () => {
    const fixture = create();
    fixture.componentRef.setInput('currentLine', 4);
    fixture.detectChanges();
    editorInstance.deltaDecorations.mockClear();

    fixture.componentRef.setInput('currentLine', null);
    fixture.detectChanges();
    expect(editorInstance.deltaDecorations).toHaveBeenCalled();
  });
});
