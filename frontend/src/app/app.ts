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
  ruby: ['n = 5', 'total = 0', 'i = 0', 'while i < n', '  total += i', '  puts total', '  i += 1', 'end'].join('\n'),
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

/**
 * Exact error text the API returns for a 404 on GET /executions/:id/trace
 * (see `ErrorResponse("execution not found")` in
 * api/src/main/java/.../web/ExecutionsResource.java). Used to recognize
 * specifically "this execution genuinely no longer exists" (as opposed to
 * some other, transient error) so the boot-time self-heal below only fires
 * for that one case — see the field doc on `awaitingLastIdLoadOutcome`.
 */
const EXECUTION_NOT_FOUND_ERROR_MESSAGE = 'execution not found';

/** localStorage key persisting the user's chosen editor/panels split ratio. */
const SPLIT_RATIO_KEY = 'code2complexity.splitRatio';

/**
 * localStorage key remembering whether the C# step-through disclaimer
 * (.csharp-note, see app.html) has been collapsed by the user. Mirrors
 * SPLIT_RATIO_KEY's "read current value on boot, persist it back on every
 * change" pattern rather than a one-shot "dismissed forever" flag: toggling
 * back open (e.g. to re-read the local_N/PDB details) also persists, so a
 * later reload doesn't re-collapse something the user just chose to expand
 * again. UX audit quick win #4 — the note itself is real, useful
 * information (not decorative), it just shouldn't cost vertical space on
 * every single C# run once someone has already read it.
 */
const CSHARP_NOTE_COLLAPSED_KEY = 'code2complexity.csharpNoteDismissed';

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

  // UX audit quick win #2: the Clipboard API write can reject (permission
  // denied, insecure context/non-HTTPS origin) — previously that left the
  // button completely unchanged with zero feedback, indistinguishable from
  // "nothing happened because I didn't actually click it". shareCopyFailed
  // drives a brief "falha ao copiar" state in the same slot shareCopied
  // uses for success; shareFallbackUrl additionally surfaces the raw link
  // in a selectable field so the user can still copy it by hand.
  readonly shareCopyFailed = signal<boolean>(false);
  readonly shareFallbackUrl = signal<string | null>(null);
  private shareCopyFailedTimeout: ReturnType<typeof setTimeout> | null = null;

  // UX audit quick win #4: whether the C# step-through disclaimer is
  // collapsed to a one-line summary. Initialized from localStorage so a
  // user who has already collapsed it once doesn't see the full banner
  // (and pay its vertical-space cost) on every subsequent C# run.
  readonly csharpNoteCollapsed = signal<boolean>(this.loadCsharpNoteCollapsed());

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

  // True while waiting for the outcome of the ONE boot-time load triggered
  // by a persisted LAST_EXECUTION_ID_KEY (the "reconnect after refresh"
  // fallback, see the constructor). Consumed (set back to false) the
  // moment that load settles, whether it succeeds or fails — see the
  // self-heal effect below. Deliberately a plain field, not a signal, for
  // the exact same reason as suppressNextPersist above: the effect that
  // consumes it also needs to write it, and a tracked read would
  // re-schedule that same effect and defeat the one-shot check.
  private awaitingLastIdLoadOutcome = false;

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

  /**
   * UX audit quick win #1: whitespace-only (including empty) code is the
   * ONLY way the previously-reachable "code is required" 422 could ever
   * happen — onRun() is the sole caller of both POST /executions
   * (ExecutionsResource.create's `code.isBlank()` check) and the analysis
   * endpoint (AnalysisResource, same check) with user-controlled code, and
   * every language's starter example is always non-blank, so switching
   * languages can never produce a blank editor on its own. Disabling Run
   * on this condition (matching Java's String#isBlank() semantics: empty
   * OR all-whitespace, not just empty) therefore makes that error state
   * fully unreachable from the UI, rather than just reachable-but-prettier.
   */
  readonly isCodeBlank = computed(() => this.code().trim().length === 0);

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
      this.awaitingLastIdLoadOutcome = true;
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

    // Self-heals a stale LAST_EXECUTION_ID_KEY: if the boot-time reconnect
    // load above comes back 404 "execution not found" (most commonly
    // because the API's in-memory ExecutionStore was reset since this id
    // was saved — e.g. an `api` container restart/rebuild), the id is
    // wiped from localStorage so the NEXT page load starts clean instead
    // of showing this same error on every future visit until someone
    // manually clears localStorage. Gated on `isBusy()` going back to
    // false rather than on `runError()` alone, so this only fires once the
    // load has actually settled (success OR failure) — reading `runError()`
    // while the request is still in flight would consume the one-shot flag
    // on its still-null initial value. Narrowed to the exact
    // EXECUTION_NOT_FOUND_ERROR_MESSAGE (not "any error") so a transient
    // failure (network hiccup, API down) does not throw away a resume id
    // that may still be perfectly valid.
    effect(() => {
      const busy = this.isBusy();
      if (busy || !this.awaitingLastIdLoadOutcome) return;
      this.awaitingLastIdLoadOutcome = false; // one-shot: only the boot-time lastId load
      if (this.runError() === EXECUTION_NOT_FOUND_ERROR_MESSAGE && typeof localStorage !== 'undefined') {
        localStorage.removeItem(LAST_EXECUTION_ID_KEY);
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
      this.shareCopyFailed.set(false);
      this.shareFallbackUrl.set(null);
      if (this.shareCopyFailedTimeout) clearTimeout(this.shareCopyFailedTimeout);
      this.shareCopied.set(true);
      if (this.shareCopiedTimeout) clearTimeout(this.shareCopiedTimeout);
      this.shareCopiedTimeout = setTimeout(() => this.shareCopied.set(false), 2000);
    } catch {
      // Clipboard API rejected (permission denied, insecure/non-HTTPS
      // context, etc.) — surface it instead of leaving the button
      // unchanged with no feedback, and keep the raw URL visible/selectable
      // as a manual-copy fallback since the automatic path just failed.
      this.shareCopied.set(false);
      this.shareCopyFailed.set(true);
      this.shareFallbackUrl.set(url);
      if (this.shareCopyFailedTimeout) clearTimeout(this.shareCopyFailedTimeout);
      this.shareCopyFailedTimeout = setTimeout(() => this.shareCopyFailed.set(false), 4000);
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

  /**
   * Flips the C# disclaimer between its full text and a compact one-line
   * summary, persisting the new state so it's remembered on the next visit
   * (see CSHARP_NOTE_COLLAPSED_KEY's doc comment for why this persists both
   * directions instead of a one-shot "dismissed forever" flag).
   */
  toggleCsharpNote(): void {
    const collapsed = !this.csharpNoteCollapsed();
    this.csharpNoteCollapsed.set(collapsed);
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem(CSHARP_NOTE_COLLAPSED_KEY, String(collapsed));
    }
  }

  private loadCsharpNoteCollapsed(): boolean {
    if (typeof localStorage === 'undefined') return false;
    return localStorage.getItem(CSHARP_NOTE_COLLAPSED_KEY) === 'true';
  }
}
