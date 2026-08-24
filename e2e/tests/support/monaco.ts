import type { Page } from '@playwright/test';

/**
 * Replaces the whole content of the app's Monaco editor with `code`.
 *
 * Two real, empirically-confirmed races make the naive
 * `click → press(Meta/Control+a) → keyboard.type(code)` sequence flaky:
 *
 * 1. `keyboard.type` sends one keydown per character. For code containing
 *    several brace/paren/quote characters, Monaco's per-keystroke handling
 *    (auto-closing pairs, electric-character reindent) can desync from the
 *    selection-replace operation, silently leaving the tail of the
 *    original buffer in place below whatever got typed — confirmed by
 *    dumping `.view-line` contents after typing. `insertText` inserts the
 *    whole string as one atomic edit, like a real paste, avoiding this.
 * 2. The select-all keybinding (`editor.action.selectAll`) is not always
 *    processed by the time the very next action fires — confirmed by
 *    checking for Monaco's own selection-overlay DOM (`.selected-text`,
 *    rendered by `SelectionsOverlay`) right after pressing it: sometimes
 *    no selection has rendered yet, and the next `insertText` then lands
 *    at a stray cursor position instead of replacing the buffer. Retrying
 *    the keypress until the overlay actually appears removes the race.
 */
export async function replaceEditorContent(page: Page, code: string): Promise<void> {
  await page.locator('.view-line').first().click();

  const selectAllKey = process.platform === 'darwin' ? 'Meta+a' : 'Control+a';
  let selected = false;
  for (let attempt = 0; attempt < 10 && !selected; attempt++) {
    await page.keyboard.press(selectAllKey);
    await page.waitForTimeout(30);
    selected = (await page.locator('.monaco-editor .selected-text').count()) > 0;
  }
  if (!selected) {
    throw new Error('replaceEditorContent: select-all never rendered a selection');
  }

  await page.keyboard.insertText(code);
}
