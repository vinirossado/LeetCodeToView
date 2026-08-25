import {
  AfterViewInit,
  Component,
  ElementRef,
  OnDestroy,
  effect,
  input,
  output,
  signal,
  viewChild,
} from '@angular/core';
import * as monaco from 'monaco-editor';
// Monaco's real, internal `tabFocusMode` singleton (browser/config/
// tabFocus.js — NOT part of the public `monaco.*` API surface; ambient
// typing for it lives in src/types/monaco-internal.d.ts, since monaco-editor
// only ships a single bundled editor.api.d.ts, not per-file types).
// Imported directly instead of going through the "official", documented
// path (triggering `editor.action.toggleTabFocusMode` via
// `editor.trigger(...)`, or the real Ctrl+M / Ctrl+Shift+M keybinding it's
// bound to) because, in THIS exact production build (Angular's esbuild
// pipeline bundling the `monaco-editor` npm package), that documented path
// was found to be silently non-functional — confirmed empirically against
// a real running instance: `editorInstance.getAction('editor.action.toggleTabFocusMode')`
// returns null (never reaches the editor's own `_actions` map, since
// `registerAction2`-based commands aren't `EditorAction` contributions),
// AND, more fundamentally, pressing the REAL Ctrl+M keybinding inside a
// live focused editor and then Tab still left focus trapped inside the
// editor (i.e. not even Monaco's own built-in keybinding path works here,
// not just this component's programmatic call) — see the regression test
// this finding produced, e2e/tests/keyboard-accessibility.spec.ts.
// `editorConfiguration.js` (the module that actually computes the
// `tabFocusMode` EDITOR OPTION every instance reads) sources its value
// from this EXACT singleton (`tabFocusMode: TabFocus.getTabFocusMode()`),
// so importing it directly is not a workaround around Monaco's real
// mechanism — since a deep import resolves to the identical file
// editorConfiguration.js itself imports, this IS Monaco's real mechanism,
// just reached directly instead of through the broken command-registry
// indirection.
import { TabFocus } from 'monaco-editor/esm/vs/editor/browser/config/tabFocus.js';
import type { Language } from '../../core/models/language';

/**
 * localStorage key persisting whether Tab moves focus out of the editor
 * (Monaco's built-in "Tab Moves Focus" mode) instead of inserting a tab
 * character. Follows this app's `code2complexity.<name>` convention (see
 * app.ts's LAST_EXECUTION_ID_KEY/SPLIT_RATIO_KEY).
 *
 * WCAG 2.1.2 "No Keyboard Trap" fix: Monaco's own default (tabFocusMode =
 * false, toggled only via the undiscoverable Ctrl+M / Cmd+Shift+M shortcut,
 * see https://github.com/microsoft/monaco-editor/wiki/Monaco-Editor-Accessibility-Guide)
 * traps a keyboard-only user inside the editor permanently on first Tab —
 * confirmed via a real Playwright pass (25+ consecutive Tabs never escaped
 * the editor, so playback controls/breakpoints/panel tabs were unreachable).
 * This app instead DEFAULTS tabFocusMode to true (Tab moves focus onward,
 * same as every other control on the page) so a keyboard-only user is never
 * trapped without already knowing Monaco's internals, and shows a small
 * persistent toggle next to the editor (see the template) so a
 * code-writing user who wants Tab-for-indent can switch it back — the
 * choice then persists across reloads via TAB_FOCUS_MODE_KEY.
 */
const TAB_FOCUS_MODE_KEY = 'code2complexity.tabFocusMode';

// NOTE: no `MonacoEnvironment.getWorker` is configured here. Monaco's web
// workers are only needed for language *services* (autocomplete/diagnostics)
// for languages like TS/JSON/CSS — Java/C#/Ruby get plain Monarch-grammar
// syntax highlighting, which works fine on the main thread. Wiring up worker
// bundling through @angular/build's esbuild pipeline turned out to be more
// trouble than it's worth for what this editor actually needs; Monaco logs a
// harmless "falling back to loading web worker code in main thread" warning
// instead of failing. Revisit if language services are ever needed.
function toMonacoLanguage(language: Language): string {
  // Monaco's built-in language ids happen to match ours exactly (including
  // 'ruby', also a built-in Monarch grammar — confirmed by checking the
  // syntax highlighting actually renders in a real run, not assumed just
  // because the id string matches).
  switch (language) {
    case 'csharp':
      return 'csharp';
    case 'ruby':
      return 'ruby';
    case 'java':
      return 'java';
  }
}

/**
 * Monaco-backed code editor. Deliberately thin: all execution/trace state
 * lives in TraceStoreService, not here — this component only renders the
 * source, highlights whatever line it's told to highlight, and reports
 * gutter clicks (breakpoint toggles) and content edits upward.
 */
@Component({
  selector: 'app-code-editor',
  standalone: true,
  templateUrl: './code-editor.component.html',
  styleUrl: './code-editor.component.css',
})
export class CodeEditorComponent implements AfterViewInit, OnDestroy {
  readonly language = input.required<Language>();
  readonly value = input<string>('');
  readonly currentLine = input<number | null>(null);
  /** True when `currentLine` was reached by actually matching a breakpoint (vs. stepping/jumping there normally). */
  readonly stoppedAtBreakpoint = input<boolean>(false);
  readonly breakpoints = input<ReadonlySet<number>>(new Set());
  readonly readOnly = input<boolean>(false);

  readonly valueChange = output<string>();
  readonly breakpointToggle = output<number>();

  private readonly host = viewChild.required<ElementRef<HTMLDivElement>>('host');
  private editorInstance: monaco.editor.IStandaloneCodeEditor | null = null;
  private lineDecorationIds: string[] = [];
  private breakpointDecorationIds: string[] = [];

  /**
   * Mirrors Monaco's global `tabFocusMode` state for the template's visible
   * toggle (see TAB_FOCUS_MODE_KEY's doc comment). Defaults to true so the
   * toggle renders correctly-labeled even before ngAfterViewInit creates
   * the actual editor instance.
   */
  readonly tabFocusMode = signal<boolean>(true);

  /** Mirrors Monaco's real mac-specific keybinding (Control+Shift+M — the physical Ctrl key, not Cmd, same "WinCtrl" distinction VS Code itself uses) vs. Ctrl+M elsewhere. Used only to label the visible toggle. */
  readonly tabFocusShortcutLabel =
    typeof navigator !== 'undefined' && /Mac|iPod|iPhone|iPad/.test(navigator.platform ?? '')
      ? 'Control+Shift+M'
      : 'Ctrl+M';

  constructor() {
    effect(() => {
      const language = this.language();
      const editorInstance = this.editorInstance;
      if (!editorInstance) return;
      const model = editorInstance.getModel();
      if (model) monaco.editor.setModelLanguage(model, toMonacoLanguage(language));
    });

    effect(() => {
      const line = this.currentLine();
      const stoppedAtBreakpoint = this.stoppedAtBreakpoint();
      this.applyCurrentLineDecoration(line, stoppedAtBreakpoint);
    });

    effect(() => {
      const breakpoints = this.breakpoints();
      this.applyBreakpointDecorations(breakpoints);
    });

    // Reacts to `value` changing *after* creation — e.g. the language
    // selector swapping in a starter example. Guarded against the model
    // already holding this text so the `onDidChangeModelContent` this
    // triggers doesn't bounce back through `valueChange` as a no-op churn.
    effect(() => {
      const value = this.value();
      const editorInstance = this.editorInstance;
      if (!editorInstance) return;
      if (editorInstance.getValue() !== value) {
        editorInstance.setValue(value);
      }
    });
  }

  ngAfterViewInit(): void {
    this.editorInstance = monaco.editor.create(this.host().nativeElement, {
      value: this.value(),
      language: toMonacoLanguage(this.language()),
      automaticLayout: true,
      minimap: { enabled: false },
      readOnly: this.readOnly(),
      glyphMargin: true,
      // Match the app's dark theme instead of leaving Monaco on its light
      // default — the redesign moved the whole surrounding UI to dark.
      theme: 'vs-dark',
    });

    this.editorInstance.onDidChangeModelContent(() => {
      this.valueChange.emit(this.editorInstance?.getValue() ?? '');
    });

    this.editorInstance.onMouseDown((e) => {
      const isGutter =
        e.target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN ||
        e.target.type === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS;
      if (isGutter && e.target.position) {
        this.breakpointToggle.emit(e.target.position.lineNumber);
      }
    });

    this.applyTabFocusMode(this.loadTabFocusMode());
  }

  /**
   * Flips Monaco's "Tab Moves Focus" mode — same effect pressing Ctrl+M /
   * Ctrl+Shift+M inside the editor is SUPPOSED to have (see this file's
   * `TabFocus` import doc comment for why that documented path was found
   * to be silently broken in this exact production build, and why calling
   * `TabFocus.setTabFocusMode` directly is used instead), just also
   * reachable from a visible on-page control instead of only a keyboard
   * shortcut a first-time user has no way to discover.
   */
  toggleTabFocusMode(): void {
    const next = !TabFocus.getTabFocusMode();
    TabFocus.setTabFocusMode(next);
    this.tabFocusMode.set(next);
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem(TAB_FOCUS_MODE_KEY, String(next));
    }
  }

  /** Applies `desired` directly. `TabFocus` is process-wide global state (shared by every Monaco instance on the page), not a per-editor option — there is no per-instance "set" to call instead. */
  private applyTabFocusMode(desired: boolean): void {
    TabFocus.setTabFocusMode(desired);
    this.tabFocusMode.set(desired);
  }

  /** Reads the persisted tabFocusMode preference — true (Tab moves focus) is this app's own default, see TAB_FOCUS_MODE_KEY's doc comment. */
  private loadTabFocusMode(): boolean {
    if (typeof localStorage === 'undefined') return true;
    const stored = localStorage.getItem(TAB_FOCUS_MODE_KEY);
    if (stored === 'false') return false;
    return true;
  }

  ngOnDestroy(): void {
    this.editorInstance?.dispose();
  }

  private applyCurrentLineDecoration(line: number | null, stoppedAtBreakpoint: boolean): void {
    if (!this.editorInstance) return;
    const decorations: monaco.editor.IModelDeltaDecoration[] =
      line === null
        ? []
        : [
            {
              range: new monaco.Range(line, 1, line, 1),
              options: {
                isWholeLine: true,
                className: stoppedAtBreakpoint ? 'current-line-breakpoint-hit' : 'current-line-highlight',
                glyphMarginClassName: stoppedAtBreakpoint
                  ? 'current-line-breakpoint-hit-glyph'
                  : 'current-line-glyph',
              },
            },
          ];
    this.lineDecorationIds = this.editorInstance.deltaDecorations(this.lineDecorationIds, decorations);
    if (line !== null) {
      this.editorInstance.revealLineInCenter(line);
    }
  }

  private applyBreakpointDecorations(breakpoints: ReadonlySet<number>): void {
    if (!this.editorInstance) return;
    const decorations: monaco.editor.IModelDeltaDecoration[] = [...breakpoints].map((line) => ({
      range: new monaco.Range(line, 1, line, 1),
      options: {
        isWholeLine: false,
        glyphMarginClassName: 'breakpoint-glyph',
      },
    }));
    this.breakpointDecorationIds = this.editorInstance.deltaDecorations(
      this.breakpointDecorationIds,
      decorations,
    );
  }
}
