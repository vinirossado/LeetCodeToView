import { test as base } from '@playwright/test';
import { CodeEditorPage } from './code-editor-page';

/**
 * Extends Playwright's `test` with a ready `codePage` fixture: localStorage
 * is cleared (matches every spec's previous `test.beforeEach`) before the
 * test body runs, so tests just call `codePage.goto()` and go — no
 * per-file `beforeEach` boilerplate, no raw `page.addInitScript` calls.
 */
export const test = base.extend<{ codePage: CodeEditorPage }>({
  codePage: async ({ page }, use) => {
    const codePage = new CodeEditorPage(page);
    await codePage.clearSession();
    await use(codePage);
  },
});

export { expect } from '@playwright/test';
