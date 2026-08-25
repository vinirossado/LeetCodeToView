import { TestBed } from '@angular/core/testing';
import { beforeEach, describe, expect, it, vi } from 'vitest';

// Monaco needs real layout/canvas/workers that jsdom does not provide, and we
// only want to verify *our* wiring (create/update calls, decorations,
// gutter-click -> breakpoint toggle), not Monaco's own rendering — so the
// whole module is replaced with spies. `setValue`/`getValue` share a tiny
// bit of real state so the "only call setValue when the text actually
// differs" guard in the component can be exercised meaningfully.
let modelValue = '';
const editorInstance = {
  setValue: vi.fn((v: string) => {
    modelValue = v;
  }),
  getValue: vi.fn(() => modelValue),
  onDidChangeModelContent: vi.fn(),
  onMouseDown: vi.fn(),
  deltaDecorations: vi.fn(() => []),
  revealLineInCenter: vi.fn(),
  dispose: vi.fn(),
  getModel: vi.fn(() => ({ getLineCount: () => 10 })),
};

// Mirrors the real global Monaco singleton (browser/config/tabFocus.js) —
// see code-editor.component.ts's TabFocus import doc comment for why this
// is imported/mocked directly rather than via editor.trigger()/getAction():
// a real running instance of this exact production build showed the
// documented editor.action.toggleTabFocusMode command/keybinding path
// silently does nothing (confirmed via e2e/tests/keyboard-accessibility.spec.ts),
// so the component now reads/writes this singleton directly instead.
let fakeTabFocusMode = false;
vi.mock('monaco-editor/esm/vs/editor/browser/config/tabFocus.js', () => ({
  TabFocus: {
    getTabFocusMode: () => fakeTabFocusMode,
    setTabFocusMode: (v: boolean) => {
      fakeTabFocusMode = v;
    },
  },
}));

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
    modelValue = '';
    fakeTabFocusMode = false; // Monaco's own real default before this app applies its own
    localStorage.clear();
    TestBed.configureTestingModule({ imports: [CodeEditorComponent] });
  });

  function create() {
    const fixture = TestBed.createComponent(CodeEditorComponent);
    fixture.componentRef.setInput('language', 'java');
    fixture.componentRef.setInput('value', 'int x = 1;');
    fixture.detectChanges();
    modelValue = 'int x = 1;'; // simulate Monaco having applied the initial `value` create() option
    editorInstance.setValue.mockClear();
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

  it('maps the "ruby" language input to Monaco\'s "ruby" language id', async () => {
    const monaco = await import('monaco-editor');
    const fixture = TestBed.createComponent(CodeEditorComponent);
    fixture.componentRef.setInput('language', 'ruby');
    fixture.componentRef.setInput('value', '');
    fixture.detectChanges();
    const [, options] = (monaco.editor.create as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(options.language).toBe('ruby');
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

  it('pushes a new `value` (e.g. a starter example swapped in on language change) into the model', () => {
    const fixture = create();
    fixture.componentRef.setInput('value', 'for (int i = 0; i < 5; i++) {}');
    fixture.detectChanges();
    expect(editorInstance.setValue).toHaveBeenCalledWith('for (int i = 0; i < 5; i++) {}');
  });

  it('does not call setValue when the model already holds that text (avoids clobbering the cursor while typing)', () => {
    const fixture = create();
    fixture.componentRef.setInput('value', 'int x = 1;'); // same as the initial value from create()
    fixture.detectChanges();
    expect(editorInstance.setValue).not.toHaveBeenCalled();
  });

  describe('tabFocusMode (WCAG 2.1.2 keyboard-trap fix)', () => {
    it('defaults tabFocusMode to true on first visit (no persisted preference) — Tab moves focus out of the editor, not Monaco\'s own trapping default', () => {
      const fixture = create();
      expect(fakeTabFocusMode).toBe(true);
      expect(fixture.componentInstance.tabFocusMode()).toBe(true);
    });

    it('respects a persisted "false" preference (user explicitly chose Tab-for-indent)', () => {
      localStorage.setItem('code2complexity.tabFocusMode', 'false');
      const fixture = create();
      expect(fakeTabFocusMode).toBe(false);
      expect(fixture.componentInstance.tabFocusMode()).toBe(false);
    });

    it('toggleTabFocusMode() flips the mode and persists the new choice', () => {
      const fixture = create();
      expect(fixture.componentInstance.tabFocusMode()).toBe(true);

      fixture.componentInstance.toggleTabFocusMode();

      expect(fixture.componentInstance.tabFocusMode()).toBe(false);
      expect(localStorage.getItem('code2complexity.tabFocusMode')).toBe('false');

      fixture.componentInstance.toggleTabFocusMode();

      expect(fixture.componentInstance.tabFocusMode()).toBe(true);
      expect(localStorage.getItem('code2complexity.tabFocusMode')).toBe('true');
    });
  });
});
