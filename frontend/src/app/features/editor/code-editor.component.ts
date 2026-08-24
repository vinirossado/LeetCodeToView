import {
  AfterViewInit,
  Component,
  ElementRef,
  OnDestroy,
  effect,
  input,
  output,
  viewChild,
} from '@angular/core';
import * as monaco from 'monaco-editor';
import type { Language } from '../../core/models/language';

// NOTE: no `MonacoEnvironment.getWorker` is configured here. Monaco's web
// workers are only needed for language *services* (autocomplete/diagnostics)
// for languages like TS/JSON/CSS — Java/C# get plain Monarch-grammar syntax
// highlighting, which works fine on the main thread. Wiring up worker
// bundling through @angular/build's esbuild pipeline turned out to be more
// trouble than it's worth for what this editor actually needs; Monaco logs a
// harmless "falling back to loading web worker code in main thread" warning
// instead of failing. Revisit if language services are ever needed.
function toMonacoLanguage(language: Language): string {
  // Monaco's built-in language ids happen to match ours exactly.
  return language === 'csharp' ? 'csharp' : 'java';
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
  readonly breakpoints = input<ReadonlySet<number>>(new Set());
  readonly readOnly = input<boolean>(false);

  readonly valueChange = output<string>();
  readonly breakpointToggle = output<number>();

  private readonly host = viewChild.required<ElementRef<HTMLDivElement>>('host');
  private editorInstance: monaco.editor.IStandaloneCodeEditor | null = null;
  private lineDecorationIds: string[] = [];
  private breakpointDecorationIds: string[] = [];

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
      this.applyCurrentLineDecoration(line);
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
  }

  ngOnDestroy(): void {
    this.editorInstance?.dispose();
  }

  private applyCurrentLineDecoration(line: number | null): void {
    if (!this.editorInstance) return;
    const decorations: monaco.editor.IModelDeltaDecoration[] =
      line === null
        ? []
        : [
            {
              range: new monaco.Range(line, 1, line, 1),
              options: {
                isWholeLine: true,
                className: 'current-line-highlight',
                glyphMarginClassName: 'current-line-glyph',
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
