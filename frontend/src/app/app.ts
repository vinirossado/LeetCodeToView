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

/**
 * Query param carrying a shared execution id, e.g. `?execution=<uuid>`
 * (tasks.md "Compartilhamento de execuções"). A plain query param on the
 * existing single, un-routed page was chosen over introducing a real
 * Angular Router route for this: there is no router usage anywhere in the
 * app today (app.routes.ts is an empty array, no <router-outlet> in
 * app.html) and standing up a whole route tree would be a disproportionate
 * structural change just to read one id out of the URL on boot.
 */
const SHARE_EXECUTION_QUERY_PARAM = 'execution';

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

  // Brief "copied!" confirmation shown on the share button after a
  // successful clipboard write; reverts on its own after SHARE_COPY_FEEDBACK_MS.
  readonly shareCopied = signal<boolean>(false);
  private shareCopiedTimeout: ReturnType<typeof setTimeout> | null = null;

  // Set for exactly one effect run right after booting from a shared link
  // (?execution=<id>), so that one-time load does not get written into
  // LAST_EXECUTION_ID_KEY — see the constructor comment for why. Deliberately
  // a plain field, NOT a signal: the persist effect below both reads AND
  // writes this flag in the same run, and reading a signal makes the effect
  // track it as a dependency — writing it would then re-schedule the same
  // effect to run again immediately (within the same flush), by which point
  // the flag already reads false and the "one-shot" skip would defeat
  // itself. A plain field read inside the effect body is not tracked, so
  // toggling it doesn't cause a spurious extra run.
  private suppressNextPersist = false;

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
  readonly isPlaying = this.trace.isPlaying;

  readonly currentLine = computed(() => this.currentStep()?.line ?? null);
  readonly currentStack = computed(() => this.currentStep()?.stack ?? null);
  readonly outputErrorMessage = computed(() => {
    const terminal = this.terminalEvent();
    return terminal?.type === 'error' ? terminal.message : null;
  });

  constructor() {
    // A shared link (?execution=<id>) always wins over the localStorage
    // "resume my own last run" fallback: someone who was sent a link wants
    // to see THAT execution, not silently land on whatever this browser
    // happened to run last.
    const sharedId =
      typeof window !== 'undefined' ? new URLSearchParams(window.location.search).get(SHARE_EXECUTION_QUERY_PARAM) : null;
    const lastId = typeof localStorage !== 'undefined' ? localStorage.getItem(LAST_EXECUTION_ID_KEY) : null;

    if (sharedId) {
      // Design decision (tasks.md "Compartilhamento de execuções"): opening
      // a shared link does NOT overwrite this browser's own "last
      // execution" resume slot. Sharing is treated as a one-off guest
      // visit, not a takeover of "what this browser was doing" — if it
      // silently replaced LAST_EXECUTION_ID_KEY, a user who opens a
      // colleague's link, closes the tab, and later reopens the bare app
      // URL from a bookmark would unexpectedly land back on the
      // colleague's (possibly already-expired, since ExecutionStore is
      // in-memory only) execution instead of resuming their own work. The
      // query param itself already makes a *refresh of the shared URL*
      // keep showing the shared execution (it's re-read on every boot,
      // see above) — this suppression only affects what happens once the
      // user navigates away from that URL. If the user then clicks Run or
      // opens another link, that new id persists normally.
      this.suppressNextPersist = true;
      this.session.load(sharedId);
    } else if (lastId) {
      this.session.load(lastId);
    }

    // Persist as soon as the id is known (before the run necessarily
    // finishes) so a reload can pick the trace back up via GET /trace
    // instead of losing it (spec.md "Reconexão").
    effect(() => {
      const id = this.executionId();
      if (!id) return;
      if (this.suppressNextPersist) {
        this.suppressNextPersist = false; // one-shot: only skips the shared-link boot load
        return;
      }
      if (typeof localStorage !== 'undefined') {
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

  togglePlay(): void {
    this.trace.togglePlay();
  }

  /**
   * Builds the shareable URL for the current execution
   * (`https://.../?execution=<id>`). Reads the current origin/pathname off
   * `window.location` rather than hardcoding a host, so it works the same
   * in dev (localhost:4200) and in whatever production host this actually
   * gets deployed to. Any pre-existing query string is dropped — a shared
   * link should point at exactly one execution, not carry along params
   * from whatever was in the address bar when Share was clicked (e.g. the
   * viewer's own earlier ?execution=<other-id>).
   */
  private buildShareUrl(executionId: string): string {
    const url = new URL(window.location.href);
    url.search = '';
    url.searchParams.set(SHARE_EXECUTION_QUERY_PARAM, executionId);
    return url.toString();
  }

  /**
   * Copies the current execution's share URL to the clipboard and shows a
   * brief confirmation on the button. Honest about the two ways this can
   * not "just work": no execution yet (button isn't shown in that case,
   * see app.html), and the Clipboard API itself failing (denied permission,
   * insecure context) — in that case shareCopied is left false rather than
   * lying that it succeeded.
   */
  async onCopyShareLink(): Promise<void> {
    const id = this.executionId();
    if (!id) return;

    const url = this.buildShareUrl(id);
    try {
      await navigator.clipboard.writeText(url);
      this.shareCopied.set(true);
      if (this.shareCopiedTimeout) clearTimeout(this.shareCopiedTimeout);
      this.shareCopiedTimeout = setTimeout(() => this.shareCopied.set(false), 2000);
    } catch {
      this.shareCopied.set(false);
    }
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
