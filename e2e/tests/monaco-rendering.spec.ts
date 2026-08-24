import { expect, test } from '@playwright/test';

// Regression coverage for a real bug found this session: monaco-editor's own
// CSS was never bundled into the production build (esbuild wasn't picking up
// the package's ~100 side-effect .css imports), so the editor rendered with
// every `.view-line` in normal document flow instead of `position: absolute`
// + `top: Npx` — visually jumbled/out-of-order code. Fixed by adding
// `node_modules/monaco-editor/min/vs/editor/editor.main.css` to
// `angular.json`'s `styles` array. This suite would have caught that bug;
// it exists so it can never silently regress again.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.clear());
});

test('Monaco renders with its own CSS applied (line positioning is absolute, not stacked in document flow)', async ({
  page,
}) => {
  await page.goto('/');

  const firstViewLine = page.locator('.view-line').first();
  await expect(firstViewLine).toBeVisible();

  const position = await firstViewLine.evaluate((el) => getComputedStyle(el).position);
  expect(position).toBe('absolute');
});

test('editor renders the Java starter code lines in the correct visual (top-to-bottom) order', async ({ page }) => {
  await page.goto('/');

  // Starter code (app.ts STARTER_CODE.java) starts with the class
  // declaration and ends with the println inside the loop — if Monaco's CSS
  // is broken, view-lines lose their `top` offset and can render in the
  // wrong visual order relative to each other.
  const classLine = page.locator('.view-line', { hasText: 'public class Main' });
  const printlnLine = page.locator('.view-line', { hasText: 'System.out.println' });

  await expect(classLine).toBeVisible();
  await expect(printlnLine).toBeVisible();

  const classBox = await classLine.boundingBox();
  const printlnBox = await printlnLine.boundingBox();
  expect(classBox).not.toBeNull();
  expect(printlnBox).not.toBeNull();

  // "public class Main {" is the first line of the starter example; the
  // println is nested near the end of the loop body — it must render
  // strictly below the class declaration.
  expect(classBox!.y).toBeLessThan(printlnBox!.y);
});
