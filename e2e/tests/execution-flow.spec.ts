import { expect, test } from '@playwright/test';
import { replaceEditorContent } from './support/monaco';

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
  const output = page.locator('pre');
  // The button reads "Run" as soon as the execution's terminal status
  // lands, which can be a beat ahead of the trailing stdout event(s) for
  // the final loop iteration reaching the DOM — wait for the last expected
  // line rather than snapshotting text right away, to avoid a race with
  // that in-flight render.
  await expect(output).toContainText('10');
  const outputText = await output.innerText();

  expect(outputText).not.toContain('013610');
  expect(outputText.split('\n').map((line) => line.trim())).toEqual(
    expect.arrayContaining(['0', '1', '3', '6', '10']),
  );
});

test('does not leak the JVM\'s own container-detection warnings into the stdout panel', async ({ page }) => {
  // Regression test for a second, separate bug: nsjail's cgroup-per-jail
  // setup (a 'NSJAIL.<pid>' cgroup path instead of a normal container path)
  // makes the JVM print "[warning][os,container] Cgroup ... controller
  // path ... seems to have moved ..." on every run, for both the JDI driver
  // JVM and the target JVM it launches (java.rs). That's HotSpot's own
  // unified logging, on by default, sharing this process's real stdout
  // with the sandboxed program's output (events::run_nsjail relays both on
  // the same stream) — so it always ended up in the user-facing Saída
  // panel. Fixed by passing -Xlog:os+container=off to both JVM invocations
  // in java.rs (the heap/metaspace limits are already pinned explicitly
  // via -Xmx/-XX:MaxMetaspaceSize, not cgroup-autodetected, so silencing
  // this log tag has no behavioral effect).
  await page.goto('/');

  const runButton = page.getByRole('button', { name: /Run|Executando/ });
  await runButton.click();
  await expect(runButton).toHaveText('Run', { timeout: 20_000 });

  await page.getByRole('tab', { name: 'Saída' }).click();
  const output = page.locator('pre');
  await expect(output).toContainText('10');
  const outputText = await output.innerText();

  expect(outputText).not.toContain('Cgroup');
  expect(outputText).not.toContain('os,container');
  expect(outputText.split('\n').map((line) => line.trim())).toEqual(['0', '1', '3', '6', '10']);
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
  // Uses replaceEditorContent (support/monaco.ts) rather than a raw
  // click+press+type sequence: two separate, empirically-confirmed races
  // in the naive version made this flaky (see that helper's doc comment).
  // Notably, this test's own assertion happened to still pass even while
  // silently corrupted by the first race (the leftover Main.java filename
  // in the compiler error's location prefix satisfies
  // `.toContainText('Main')` regardless of the actual submitted code) —
  // so it was testing the wrong thing until this was found and fixed.
  await replaceEditorContent(page, 'class Solution { void run() {} }');

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

test('blocks Java code that spawns a real thread, with a clear message instead of a silent hang or a false completion', async ({
  page,
}) => {
  // Regression coverage for the multi-thread event model decision
  // (spec.md "Multi-thread", pending since Fase 1): blocked in the MVP.
  // Detected at runtime by jdi/Debugger.java (a JVM ThreadGroup check,
  // empirically validated — see tasks.md/sandbox/jdi/Debugger.java for the
  // false-positive found and fixed along the way). Java-only: the
  // equivalent C# detection (sandbox/src/com.rs) is deliberately NOT
  // covered by an E2E test here — it was empirically found to be
  // unreliable (a separate, pre-existing ICorDebug stepper stall inside
  // `Thread.Start()`'s own internals sometimes prevents the detection from
  // ever firing, documented in tasks.md/com.rs) and a flaky E2E test would
  // be worse than no test at all.
  await page.goto('/');

  // Uses replaceEditorContent (support/monaco.ts): a peer session flagged
  // this test failing deterministically with a javac syntax error instead
  // of exercising the JDI multi-thread check at all. Root-caused (not
  // assumed) with a throwaway debug test dumping `.view-line` contents
  // after the old raw click+press+type sequence: `type`'s per-keystroke
  // handling of this brace/paren/quote-heavy one-liner desynced from the
  // selection, silently leaving 9 stale lines of the starter example below
  // the typed text — so the submitted "file" never compiled. See the
  // helper's doc comment for the second, independent select-all race also
  // found and fixed in the same pass.
  await replaceEditorContent(
    page,
    'class Main { public static void main(String[] a) throws InterruptedException { ' +
      'Thread t = new Thread(() -> System.out.println("hi")); t.start(); t.join(); } }',
  );

  const runButton = page.getByRole('button', { name: /Run|Executando/ });
  await runButton.click();
  await expect(runButton).toHaveText('Run', { timeout: 20_000 });

  await expect(page.locator('.banner')).toContainText('multi-thread execution is not supported yet', {
    timeout: 10_000,
  });
});
