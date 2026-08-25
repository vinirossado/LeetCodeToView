import { expect, test } from './support/fixtures';
import { tabThrough } from './support/keyboard';

// Regression test for a real WCAG 2.1.2 "No Keyboard Trap" violation
// (Level A) found via a hands-on Playwright keyboard-only audit: pressing
// Tab from the language select reached Run, then the Monaco editor — and
// then got PERMANENTLY stuck inside the editor. 25+ consecutive Tab presses
// never escaped it, so the playback controls and all 5 panel tabs were
// completely unreachable via keyboard. Root cause: Monaco's own default
// (`tabFocusMode = false`) makes Tab insert a tab character instead of
// moving focus, with no in-UI way to discover the real fix (Ctrl+M /
// Ctrl+Shift+M — see https://github.com/microsoft/monaco-editor/wiki/Monaco-Editor-Accessibility-Guide).
//
// The fix (code-editor.component.ts): this app now DEFAULTS tabFocusMode to
// true (Tab moves focus onward, like every other control on the page) and
// shows a persistent, visible toggle next to the editor so a user who wants
// Tab-for-indent can switch it back — see tabFocusMode's doc comment there.
//
// This test proves the fix with the SAME methodology the original audit
// used to find the bug, run in reverse: start a real keyboard-only Tab
// sequence and assert it actually reaches every interactive control on the
// page in one pass, instead of just asserting the Monaco option was set.
test.describe('keyboard accessibility — Monaco "Tab Moves Focus" trap fix (WCAG 2.1.2)', () => {
  test('Tab never gets stuck inside the editor — a full keyboard-only pass reaches Run, the editor, every playback control, and all 5 panel tabs', async ({
    codePage,
    page,
  }) => {
    await codePage.goto();
    // Run once via mouse first (a real first-time user types/keeps the
    // starter code and clicks Run) so every playback control is actually
    // enabled/focusable — a *disabled* button is legitimately skipped by
    // the browser's own Tab order regardless of this bug, so testing with
    // everything enabled is what actually isolates the trap.
    await codePage.runAndWaitForFinish();

    await codePage.languageSelect.focus();

    // 60 is comfortably more than enough to cross the whole page once even
    // through the editor's own internal DOM (Monaco's textarea, current-line
    // widgets, etc.) if Tab is working correctly; the ORIGINAL bug reproduced
    // with 25+ consecutive presses never escaping at all.
    const visited = await tabThrough(page, 60);

    const reached = (matcher: (fingerprint: string) => boolean) => visited.some(matcher);

    // 1. Reaches Run at some point (it's right after the language select in
    // document order).
    expect(reached((f) => /\|Run\b/.test(f) || f.includes('run-btn'))).toBe(true);

    // 2. Actually enters the Monaco editor (proves this test exercises the
    // real trap surface, not a page where the editor never got focus at all).
    expect(reached((f) => f.includes('monaco-editor') || f.includes('inputarea'))).toBe(true);

    // 3. ESCAPES the editor and reaches its own visible tab-focus-mode
    // toggle — the concrete, discoverable fix (not just a keyboard shortcut
    // buried in Monaco's own command palette).
    expect(reached((f) => f.includes('tab-focus-toggle'))).toBe(true);

    // 4. Reaches real playback controls beyond the editor — this is
    // EXACTLY what was "completely unreachable via keyboard" before the
    // fix. Picking controls that stay ENABLED right after a completed run
    // (not e.g. "Próximo passo"/"Ir para o fim", which are legitimately
    // disabled — and so correctly skipped in Tab order by the browser
    // itself, unrelated to this bug — once the live-follow cursor is
    // sitting at the trace's own end, which is where it naturally lands
    // right after Run finishes).
    for (const title of ['Ir para o início', 'Breakpoint anterior', 'Passo anterior']) {
      expect(reached((f) => f.includes(title)), `playback control "${title}" was never focused`).toBe(true);
    }

    // 5. Reaches every one of the 5 panel tabs.
    for (const tabName of ['Variáveis', 'Call Stack', 'Saída', 'Complexidade', 'Timeline']) {
      expect(reached((f) => f.includes(tabName)), `panel tab "${tabName}" was never focused`).toBe(true);
    }

    // Not stuck: a real trap collapses the visited set down to just 1-2
    // distinct elements repeating forever (confirmed empirically pre-fix —
    // 25+ Tabs, all landing back on the same Monaco textarea). Reaching this
    // many genuinely DISTINCT fingerprints in 60 presses is only possible if
    // focus kept moving across the whole page.
    const distinct = new Set(visited);
    expect(distinct.size).toBeGreaterThan(10);
  });

  test('the tab-focus-mode toggle is visible next to the editor, defaults to "Tab moves focus", and can be switched to "Tab indents code"', async ({
    codePage,
    page,
  }) => {
    await codePage.goto();

    await expect(codePage.tabFocusToggle).toBeVisible();
    await expect(codePage.tabFocusToggle).toContainText('Tab move o foco');

    await codePage.tabFocusToggle.click();
    await expect(codePage.tabFocusToggle).toContainText('Tab indenta código');

    // Persists (localStorage, code2complexity.tabFocusMode) — read the
    // storage directly rather than reloading the page: the `codePage`
    // fixture's clearSession() registers an init script that wipes
    // localStorage on EVERY navigation of this page (by design, so each
    // test starts from a clean slate — see support/fixtures.ts), so a
    // second `goto()`/`page.reload()` here would just re-clear the very
    // value this assertion means to check, not exercise anything real.
    const stored = await page.evaluate(() => localStorage.getItem('code2complexity.tabFocusMode'));
    expect(stored).toBe('false');
  });

  test('with "Tab indents code" selected, pressing Tab inside the editor inserts a tab character instead of moving focus (code-writing users are not broken by the new default)', async ({
    codePage,
    page,
  }) => {
    await codePage.goto();
    await codePage.tabFocusToggle.click();
    await expect(codePage.tabFocusToggle).toContainText('Tab indenta código');

    await codePage.firstViewLine.click();
    await page.keyboard.press('Tab');

    // Focus must still be inside the editor (Tab did NOT move it away).
    const stillInEditor = await page.evaluate(() =>
      document.activeElement?.classList.contains('inputarea'),
    );
    expect(stillInEditor).toBe(true);
  });
});
