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

  // The starter example prints a running total once per loop iteration
  // (n = 5), so stdout is "0", "1", "3", "6", "10" on separate lines (see
  // app.ts STARTER_CODE.java).
  await page.getByRole('tab', { name: 'Saída' }).click();
  const output = page.locator('pre');
  await expect(output).toContainText('0');
});

test('renders each stdout line on its own line instead of running them together', async ({ page }) => {
  // Regression test: outputSoFar() (trace-store.service.ts) used to join
  // stdout events with '' instead of '\n'. The API wraps sandbox-runner's
  // stdout one line at a time via BufferedReader.readLine() (ExecutionJob.
  // java), which strips the newline — so joining with '' silently glued
  // every line back together with no separator at all, including nsjail's
  // own startup diagnostics (JVM "[warning][os,container] Cgroup ..."
  // lines) that happen to share the same stdout stream as the sandboxed
  // program. The visible symptom was the Saída panel showing those warning
  // lines concatenated back-to-back, followed by the program's real
  // "0","1","3","6","10" output squashed into a single unreadable
  // "013610".
  await page.goto('/');

  const runButton = page.getByRole('button', { name: /Run|Executando/ });
  await runButton.click();
  await expect(runButton).toHaveText('Run', { timeout: 20_000 });

  await page.getByRole('tab', { name: 'Saída' }).click();
  const outputText = await page.locator('pre').innerText();

  expect(outputText).not.toContain('013610');
  expect(outputText.split('\n').map((line) => line.trim())).toEqual(
    expect.arrayContaining(['0', '1', '3', '6', '10']),
  );
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

  // Following-live navigation lands the cursor at the trace's pseudo-end
  // position after the run finishes, which can legitimately have no locals
  // in scope (e.g. right after the loop's closing brace) — jump back to the
  // very first step, which is inside the loop body and always has locals.
  await page.getByTitle('Ir para o início').click();
  await page.getByTitle('Próximo passo').click();

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
  // Found empirically: clicking the `.view-lines` *container* (rather than
  // a specific `.view-line`) places the caret ambiguously (observed landing
  // at the very end of the buffer), and Playwright's `ControlOrMeta+a`
  // shorthand did not trigger Monaco's select-all in that state either —
  // only clicking a specific line plus the platform-explicit shortcut
  // reliably replaces the whole buffer.
  await page.locator('.view-line').first().click();
  await page.keyboard.press(process.platform === 'darwin' ? 'Meta+a' : 'Control+a');
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
