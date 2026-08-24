import { Component, ElementRef, computed, effect, inject, signal, viewChild } from '@angular/core';
import { ComplexityApiService } from './core/services/complexity-api.service';
import { ExecutionSessionService } from './core/services/execution-session.service';
import { TraceStoreService } from './core/services/trace-store.service';
import { CodeEditorComponent } from './features/editor/code-editor.component';
import { CallStackPanelComponent } from './features/panels/call-stack-panel.component';
import { ComplexityPanelComponent } from './features/panels/complexity-panel.component';
import { OutputPanelComponent } from './features/panels/output-panel.component';
import { StatusBannerComponent } from './features/panels/status-banner.component';
import { TimelineChartComponent } from './features/panels/timeline-chart.component';
import { VariablesPanelComponent } from './features/panels/variables-panel.component';
import { PlaybackControlsComponent } from './features/controls/playback-controls.component';
import { LANGUAGES, languageLabel, type Language } from './core/models/language';
import type { AnalysisOutcome } from './core/models/analysis.model';

const STARTER_CODE: Record<Language, string> = {
  java: [
    'public class Main {',
    '    public static void main(String[] args) {',
    '        int n = 5;',
    '        int total = 0;',
    '        for (int i = 0; i < n; i++) {',
    '            total += i;',
    '            System.out.println(total);',
    '        }',
    '    }',
    '}',
  ].join('\n'),
  csharp: ['int n = 5;', 'int total = 0;', 'for (int i = 0; i < n; i++) {', '    total += i;', '    Console.WriteLine(total);', '}'].join(
    '\n',
  ),
};

/** localStorage key for the reload/reconnect fallback (spec.md "Reconexão"). */
const LAST_EXECUTION_ID_KEY = 'code2complexity.lastExecutionId';

/** localStorage key persisting the user's chosen editor/panels split ratio. */
const SPLIT_RATIO_KEY = 'code2complexity.splitRatio';

/** Fraction of the layout width given to the editor column; the resizer is clamped within this range. */
const MIN_SPLIT_RATIO = 0.3;
const MAX_SPLIT_RATIO = 0.75;
const DEFAULT_SPLIT_RATIO = 0.62;

/** One of the switchable right-side panel tabs (LeetCode-style Testcase/Result tabs). */
type PanelTabId = 'variables' | 'callstack' | 'output' | 'complexity' | 'timeline';

interface PanelTab {
  readonly id: PanelTabId;
  readonly label: string;
}

/**
 * All five panels are tabbed rather than stacked, per the explicit redesign
 * request ("tabs instead of stacked panels"). Complexity is a one-shot
 * static-analysis result rather than something that changes per step, so it
 * was considered for "always visible" instead — but it lives behind its own
 * tab like the others because (a) it is still one signal, unaffected by
 * switching tabs away and back, so nothing is lost by tabbing it, and (b)
 * keeping every panel behind the same mechanism is simpler and matches what
 * was asked for. Variables defaults active since it's the panel most
 * referenced while stepping through a trace (mirrors LeetCode defaulting to
 * its Testcase tab).
 */
const PANEL_TABS: readonly PanelTab[] = [
  { id: 'variables', label: 'Variáveis' },
  { id: 'callstack', label: 'Call Stack' },
  { id: 'output', label: 'Saída' },
  { id: 'complexity', label: 'Complexidade' },
  { id: 'timeline', label: 'Timeline' },
];

@Component({
  selector: 'app-root',
  standalone: true,
  imports: [
    CodeEditorComponent,
    VariablesPanelComponent,
    CallStackPanelComponent,
    OutputPanelComponent,
    StatusBannerComponent,
    ComplexityPanelComponent,
    TimelineChartComponent,
    PlaybackControlsComponent,
  ],
  templateUrl: './app.html',
  styleUrl: './app.css',
})
export class App {
  private readonly session = inject(ExecutionSessionService);
  private readonly trace = inject(TraceStoreService);
  private readonly complexityApi = inject(ComplexityApiService);

  readonly languages = LANGUAGES;
  readonly languageLabel = languageLabel;

  readonly language = signal<Language>('java');
  readonly code = signal<string>(STARTER_CODE.java);

  readonly panelTabs = PANEL_TABS;
  readonly activeTab = signal<PanelTabId>('variables');

  private readonly layoutHost = viewChild<ElementRef<HTMLElement>>('layoutHost');
  readonly splitRatio = signal<number>(this.loadSplitRatio());
  readonly isResizing = signal<boolean>(false);

  readonly analysisOutcome = signal<AnalysisOutcome | null>(null);
  readonly analysisLoading = signal<boolean>(false);

  // Execution/session state, re-exposed as plain signals for the template.
  readonly executionId = this.session.executionId;
  readonly runError = this.session.runError;
  readonly isBusy = this.session.isBusy;

  // Trace navigation state, re-exposed from TraceStoreService.
  readonly hasStarted = this.trace.hasStarted;
  readonly atEnd = this.trace.atEnd;
  readonly totalSteps = this.trace.totalSteps;
  readonly steps = this.trace.steps;
  readonly currentStepIndex = this.trace.currentStepIndex;
  readonly currentStep = this.trace.currentStep;
  readonly terminalEvent = this.trace.terminalEvent;
  readonly outputSoFar = this.trace.outputSoFar;
  readonly breakpoints = this.trace.breakpoints;
  readonly landedViaBreakpoint = this.trace.landedViaBreakpoint;

  readonly currentLine = computed(() => this.currentStep()?.line ?? null);
  readonly currentStack = computed(() => this.currentStep()?.stack ?? null);
  readonly outputErrorMessage = computed(() => {
    const terminal = this.terminalEvent();
    return terminal?.type === 'error' ? terminal.message : null;
  });

  constructor() {
    const lastId = typeof localStorage !== 'undefined' ? localStorage.getItem(LAST_EXECUTION_ID_KEY) : null;
    if (lastId) {
      this.session.load(lastId);
    }

    // Persist as soon as the id is known (before the run necessarily
    // finishes) so a reload can pick the trace back up via GET /trace
    // instead of losing it (spec.md "Reconexão").
    effect(() => {
      const id = this.executionId();
      if (id && typeof localStorage !== 'undefined') {
        localStorage.setItem(LAST_EXECUTION_ID_KEY, id);
      }
    });
  }

  onLanguageChange(event: Event): void {
    const language = (event.target as HTMLSelectElement).value as Language;
    this.language.set(language);
    // Switching language swaps in that language's starter example — the
    // previous code is almost certainly not valid in the new language
    // anyway (different syntax entirely), so there is no useful "keep the
    // edits" option here.
    this.code.set(STARTER_CODE[language]);
  }

  onRun(): void {
    const language = this.language();
    const code = this.code();

    this.session.run(language, code);
    this.runAnalysis(language, code);
  }

  onBreakpointToggle(line: number): void {
    this.trace.toggleBreakpoint(line);
  }

  stepForward(): void {
    this.trace.stepForward();
  }

  stepBack(): void {
    this.trace.stepBack();
  }

  jumpToStart(): void {
    this.trace.jumpToStart();
  }

  jumpToEnd(): void {
    this.trace.jumpToEnd();
  }

  runToNextBreakpoint(): void {
    this.trace.runToNextBreakpoint();
  }

  runToPreviousBreakpoint(): void {
    this.trace.runToPreviousBreakpoint();
  }

  private runAnalysis(language: Language, code: string): void {
    this.analysisLoading.set(true);
    this.complexityApi.analyze(language, code).subscribe((outcome) => {
      this.analysisOutcome.set(outcome);
      this.analysisLoading.set(false);
    });
  }

  onTabSelect(tab: PanelTabId): void {
    this.activeTab.set(tab);
  }

  /**
   * Starts a drag-to-resize gesture on the divider between the editor
   * column and the tabbed panels column. Plain pointer events (no resize
   * library is installed — see package.json), clamped so neither side can
   * be dragged down to zero width.
   */
  onResizerPointerDown(event: PointerEvent): void {
    event.preventDefault();
    const layoutEl = this.layoutHost()?.nativeElement;
    if (!layoutEl) return;
    const rect = layoutEl.getBoundingClientRect();

    this.isResizing.set(true);

    const onPointerMove = (moveEvent: PointerEvent) => {
      const ratio = (moveEvent.clientX - rect.left) / rect.width;
      this.splitRatio.set(Math.min(MAX_SPLIT_RATIO, Math.max(MIN_SPLIT_RATIO, ratio)));
    };

    const onPointerUp = () => {
      document.removeEventListener('pointermove', onPointerMove);
      document.removeEventListener('pointerup', onPointerUp);
      this.isResizing.set(false);
      if (typeof localStorage !== 'undefined') {
        localStorage.setItem(SPLIT_RATIO_KEY, String(this.splitRatio()));
      }
    };

    document.addEventListener('pointermove', onPointerMove);
    document.addEventListener('pointerup', onPointerUp);
  }

  private loadSplitRatio(): number {
    if (typeof localStorage === 'undefined') return DEFAULT_SPLIT_RATIO;
    const stored = Number(localStorage.getItem(SPLIT_RATIO_KEY));
    if (Number.isFinite(stored) && stored >= MIN_SPLIT_RATIO && stored <= MAX_SPLIT_RATIO) {
      return stored;
    }
    return DEFAULT_SPLIT_RATIO;
  }
}
