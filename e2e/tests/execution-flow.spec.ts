import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.clear());
});

test('runs the Java starter example end to end: leaves "Executando…" and shows the program output', async ({
  page,
}) => {
  await page.goto('/');

  const runButton = page.getByRole('button', { name: /Run|Executando/ });
  await runButton.click();

  // Regression coverage for two real bugs found this session, both of which
  // manifested as the button staying on "Executando…" forever:
  // (1) `new WebSocket(url)` does not resolve a relative URL against the
  //     page (unlike fetch/<a href>) — a bare '/executions/:id/events' path
  //     threw synchronously and the resulting fallback never updated status.
  // (2) A stale `execution_id` from a previous localStorage session pointed
  //     at an execution no longer in the API's in-memory store; the 404
  //     error handler never moved status off 'pending'.
  // This assertion is the actual product requirement: the button MUST
  // eventually read "Run" again, not get stuck.
  await expect(runButton).toHaveText('Run', { timeout: 20_000 });

  const executionId = page.locator('.execution-id');
  await expect(executionId).toContainText('execution_id:');

  // The starter example prints 0..4 (see app.ts STARTER_CODE.java: a
  // running total printed once per loop iteration, n = 5).
  await page.getByRole('tab', { name: 'Saída' }).click();
  const output = page.locator('pre');
  await expect(output).toContainText('0');
});

test('runs the C# starter example end to end and shows the known local_N/PDB disclaimer', async ({ page }) => {
  await page.goto('/');

  await page.locator('.lang-select').selectOption('csharp');

  // Switching language swaps in that language's starter example (app.ts
  // onLanguageChange) and shows the C#-specific disclaimer about the
  // locals/line asymmetry (no PDB reading yet — see spec.md).
  await expect(page.locator('.csharp-note')).toContainText('local_N');

  const runButton = page.getByRole('button', { name: /Run|Executando/ });
  await runButton.click();
  await expect(runButton).toHaveText('Run', { timeout: 20_000 });

  await page.getByRole('tab', { name: 'Variáveis' }).click();
  // C# locals render as positional local_0/local_1/... placeholders, not
  // real variable names — this is the documented, intentional asymmetry.
  await expect(page.locator('dl')).toContainText('local_');
});

test('shows a clear error instead of getting stuck when a stale execution_id no longer exists server-side', async ({
  page,
}) => {
  // Regression test for the third "Executando…" bug found this session:
  // the app auto-loads whatever execution_id is in localStorage on boot
  // (spec.md "Reconexão"). Since the API's ExecutionStore is in-memory
  // only, any execution_id surviving a container recreation points at
  // nothing — GET /trace 404s, and the fix (ExecutionSessionService.load's
  // error handler) must move status to 'failed', not leave it 'pending'.
  await page.addInitScript(() => {
    localStorage.setItem('code2complexity.lastExecutionId', 'this-id-does-not-exist-anymore');
  });

  await page.goto('/');

  const runButton = page.getByRole('button', { name: /Run|Executando/ });
  await expect(runButton).toHaveText('Run', { timeout: 10_000 });
  await expect(page.locator('.error')).toContainText('execution not found');
});

test('rejects Java code without a class named Main with a clear inline error, not a silent hang', async ({
  page,
}) => {
  await page.goto('/');

  // The Monaco editor's content is what gets submitted — select all and
  // replace with code missing the required `class Main` (see
  // ExecutionsResource's server-side validation, api/src/main/java/.../web/ExecutionsResource.java).
  // `.inputarea` (not `.view-lines`) is Monaco's actual hidden textarea that
  // receives keyboard focus — clicking the rendered lines alone left focus
  // ambiguous and Ctrl+A/typing appended after the starter code instead of
  // replacing it.
  await page.locator('.inputarea').click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type('class Solution { void run() {} }');

  await page.getByRole('button', { name: 'Run' }).click();

  await expect(page.locator('.error')).toContainText('Main', { timeout: 10_000 });
});

test('switches between all five panel tabs', async ({ page }) => {
  await page.goto('/');

  const tabs = ['Variáveis', 'Call Stack', 'Saída', 'Complexidade', 'Timeline'];
  for (const tab of tabs) {
    await page.getByRole('tab', { name: tab }).click();
    await expect(page.getByRole('tab', { name: tab })).toHaveAttribute('aria-selected', 'true');
  }
});
