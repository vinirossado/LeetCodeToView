import type { Page } from '@playwright/test';

/**
 * Replaces the whole content of the app's Monaco editor with `code`.
 *
 * Three real, empirically-confirmed races/quirks make the naive
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
 * 3. A SINGLE `insertText` call for genuinely MULTI-line content (embedded
 *    `\n`s) hits a THIRD, independent issue: Monaco reformats a multi-line
 *    insert's indentation (`autoIndent`) while ALSO applying its
 *    auto-closing-brackets behavior to any `{` in it — the two interact
 *    badly, confirmed empirically by dumping `.view-line` contents
 *    afterward on real brace-nested code: indentation compounds
 *    (each subsequent line growing deeper than intended) AND an
 *    auto-inserted matching `}` from an EARLIER `{` is silently left
 *    behind as an extra, unbalanced brace once the code's OWN later `}`
 *    is also inserted (the "type over an auto-closed bracket" logic only
 *    applies to real per-keystroke typing, not a `}` embedded inside a
 *    bulk `insertText` call). This corrupts brace-heavy multi-line source
 *    silently — the editor LOOKS populated, but doesn't compile. Multi-line
 *    content is therefore inserted one line at a time instead: a real
 *    Enter keypress between lines (clearing whatever auto-indent Monaco
 *    put on the fresh line first, so only OUR line's own leading
 *    whitespace survives), and an explicit Delete right after any line
 *    ending in `{` to consume Monaco's auto-closed `}` before it can
 *    accumulate.
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

  const lines = code.split('\n');
  if (lines.length === 1) {
    await page.keyboard.insertText(code);
    return;
  }

  for (let i = 0; i < lines.length; i++) {
    if (i > 0) {
      await page.keyboard.press('Enter');
      await page.keyboard.press('Shift+Home');
      await page.keyboard.press('Delete');
    }
    await page.keyboard.insertText(lines[i]);
    if (lines[i].trim().endsWith('{')) {
      await page.keyboard.press('Delete'); // consume Monaco's auto-inserted matching '}'
    }
  }
}
