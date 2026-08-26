import { expect, type Locator, type Page } from '@playwright/test';
import { replaceEditorContent } from './monaco';

/**
 * Page Object for the app's single page (editor + playback controls + tabbed
 * panels). Every selector this suite needs lives here, once — spec files
 * call actions/read locators through this class instead of repeating
 * `page.locator(...)`/`page.getByRole(...)` calls, so a markup/selector
 * change only needs updating in one place.
 */
export class CodeEditorPage {
  constructor(private readonly page: Page) {}

  // --- Locators ---

  get runButton(): Locator {
    return this.page.getByRole('button', { name: /Run|Executando/ });
  }

  get executionId(): Locator {
    return this.page.locator('.execution-id');
  }

  get output(): Locator {
    return this.page.locator('pre');
  }

  get errorMessage(): Locator {
    return this.page.locator('.error');
  }

  get banner(): Locator {
    return this.page.locator('.banner');
  }

  get languageSelect(): Locator {
    return this.page.locator('.lang-select');
  }

  /** Visible "Tab moves focus / Tab indenta código" toggle next to the editor (WCAG 2.1.2 keyboard-trap fix). */
  get tabFocusToggle(): Locator {
    return this.page.locator('.tab-focus-toggle');
  }

  callStackFrameButtons(): Locator {
    return this.page.locator('button.frame-btn');
  }

  get frameContextLabel(): Locator {
    return this.page.locator('.frame-context');
  }

  get variables(): Locator {
    return this.page.locator('dl');
  }

  get breakpointGlyphs(): Locator {
    return this.page.locator('.breakpoint-glyph');
  }

  get currentLineBreakpointHit(): Locator {
    return this.page.locator('.current-line-breakpoint-hit');
  }

  get currentLineHighlight(): Locator {
    return this.page.locator('.current-line-highlight');
  }

  get firstViewLine(): Locator {
    return this.page.locator('.view-line').first();
  }

  viewLineContaining(text: string): Locator {
    return this.page.locator('.view-line', { hasText: text });
  }

  tab(name: string): Locator {
    return this.page.getByRole('tab', { name });
  }

  private lineNumberGutter(line: number): Locator {
    return this.page.locator('.line-numbers', { hasText: new RegExp(`^${line}$`) });
  }

  private playbackControl(title: string): Locator {
    return this.page.getByTitle(title);
  }

  get speedSelect(): Locator {
    return this.page.locator('.speed-select select');
  }

  // --- Setup actions (page state before navigation) ---

  /** Registers the init script clearing localStorage — must run before {@link goto}. */
  async clearSession(): Promise<void> {
    await this.page.addInitScript(() => localStorage.clear());
  }

  /** Seeds a (possibly stale/nonexistent) execution id, as if resuming a previous session. */
  async setStoredExecutionId(executionId: string): Promise<void> {
    await this.page.addInitScript((id) => {
      localStorage.setItem('code2complexity.lastExecutionId', id);
    }, executionId);
  }

  async goto(): Promise<void> {
    await this.page.goto('/');
  }

  // --- Actions ---

  async replaceCode(code: string): Promise<void> {
    await replaceEditorContent(this.page, code);
  }

  async selectLanguage(language: string): Promise<void> {
    await this.languageSelect.selectOption(language);
  }

  async run(): Promise<void> {
    await this.runButton.click();
  }

  async waitForRunToFinish(): Promise<void> {
    await expect(this.runButton).toHaveText('Run', { timeout: 20_000 });
  }

  async runAndWaitForFinish(): Promise<void> {
    await this.run();
    await this.waitForRunToFinish();
  }

  async openTab(name: string): Promise<void> {
    await this.tab(name).click();
  }

  async toggleBreakpointOnLine(line: number): Promise<void> {
    await this.lineNumberGutter(line).click();
  }

  async goToStart(): Promise<void> {
    await this.playbackControl('Ir para o início').click();
  }

  async goToEnd(): Promise<void> {
    await this.playbackControl('Ir para o fim').click();
  }

  async stepForward(): Promise<void> {
    await this.playbackControl('Próximo passo').click();
  }

  async nextBreakpoint(): Promise<void> {
    await this.playbackControl('Próximo breakpoint').click();
  }

  async togglePlay(): Promise<void> {
    await this.page.getByTitle(/Reproduzir automaticamente|Pausar reprodução automática/).click();
  }

  async setPlaybackSpeed(speed: '1' | '0.75' | '0.5' | '0.25'): Promise<void> {
    await this.speedSelect.selectOption(speed);
  }

  // --- Assertion helpers (state, not raw locators) ---

  async expectStepForwardEnabled(): Promise<void> {
    await expect(this.playbackControl('Próximo passo')).toBeEnabled();
  }

  async expectStepForwardDisabled(): Promise<void> {
    await expect(this.playbackControl('Próximo passo')).toBeDisabled();
  }

  async expectGoToEndEnabled(): Promise<void> {
    await expect(this.playbackControl('Ir para o fim')).toBeEnabled();
  }

  async expectGoToEndDisabled(): Promise<void> {
    await expect(this.playbackControl('Ir para o fim')).toBeDisabled();
  }
}
