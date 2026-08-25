import { expect, test } from './support/fixtures';

test('runs the Java starter example end to end: leaves "Executando…" and shows the program output', async ({
  codePage,
}) => {
  await codePage.goto();
  await codePage.run();

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
  await codePage.waitForRunToFinish();

  await expect(codePage.executionId).toContainText('execution_id:');

  // The starter example prints a running total once per loop iteration
  // (n = 5), so stdout is "0", "1", "3", "6", "10" on separate lines (see
  // app.ts STARTER_CODE.java).
  await codePage.openTab('Saída');
  await expect(codePage.output).toContainText('0');
});

test('renders each stdout line on its own line instead of running them together', async ({ codePage }) => {
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
  await codePage.goto();
  await codePage.runAndWaitForFinish();

  await codePage.openTab('Saída');
  // The button reads "Run" as soon as the execution's terminal status
  // lands, which can be a beat ahead of the trailing stdout event(s) for
  // the final loop iteration reaching the DOM — wait for the last expected
  // line rather than snapshotting text right away, to avoid a race with
  // that in-flight render.
  await expect(codePage.output).toContainText('10');
  const outputText = await codePage.output.innerText();

  expect(outputText).not.toContain('013610');
  expect(outputText.split('\n').map((line) => line.trim())).toEqual(
    expect.arrayContaining(['0', '1', '3', '6', '10']),
  );
});

test('does not leak the JVM\'s own container-detection warnings into the stdout panel', async ({ codePage }) => {
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
  await codePage.goto();
  await codePage.runAndWaitForFinish();

  await codePage.openTab('Saída');
  await expect(codePage.output).toContainText('10');
  const outputText = await codePage.output.innerText();

  expect(outputText).not.toContain('Cgroup');
  expect(outputText).not.toContain('os,container');
  expect(outputText.split('\n').map((line) => line.trim())).toEqual(['0', '1', '3', '6', '10']);
});

test('runs the C# starter example end to end and shows the known local_N/PDB disclaimer', async ({ codePage }) => {
  await codePage.goto();
  await codePage.selectLanguage('csharp');

  // Switching language swaps in that language's starter example (app.ts
  // onLanguageChange) and shows the C#-specific disclaimer. Line-granular
  // stepping (ICorDebugStepper::StepRange, see tasks.md) fixed the "same
  // line highlighted many times in a row" artifact for the common
  // single-statement-per-line case, so the banner's wording changed — but
  // the locals/PDB fallback part it also documents (local_N when a real
  // variable name can't be resolved) is unchanged, so this substring still
  // holds.
  await expect(codePage.csharpNote).toContainText('local_N');

  await codePage.runAndWaitForFinish();

  // Following-live navigation lands the cursor at the trace's pseudo-end
  // position after the run finishes, which can legitimately have no locals
  // in scope (e.g. right after the loop's closing brace) — jump back to the
  // very first step, which is inside the loop body and always has locals.
  await codePage.goToStart();
  await codePage.stepForward();

  await codePage.openTab('Variáveis');
  // C# locals render as positional local_0/local_1/... placeholders, not
  // real variable names — this is the documented, intentional asymmetry.
  await expect(codePage.variables).toContainText('local_');
});

test('runs the Ruby starter example end to end via TracePoint: real locals and a real call stack, no PDB-style disclaimer', async ({
  codePage,
}) => {
  await codePage.goto();
  await codePage.selectLanguage('ruby');

  // Unlike C#, Ruby has no equivalent asymmetry/disclaimer to show — the
  // TracePoint driver (sandbox/ruby/driver.rb) always resolves real
  // variable names (tp.binding.local_variables), same as Java's JDI, never
  // positional local_N placeholders.
  await expect(codePage.csharpNote).toHaveCount(0);

  await codePage.runAndWaitForFinish();

  // Checked right after the run finishes (live-follow position, same as
  // the plain Java test above) — NOT after navigating elsewhere, since
  // `outputSoFar()` is cumulative only up to the currently-viewed step
  // (same semantics as the variables panel showing only the CURRENT
  // step's locals). Found the hard way: an earlier version of this test
  // checked output only after already navigating back to the start of the
  // trace and stepping forward a few times for the variables assertion
  // below — output was empty at that earlier position because the Ruby
  // starter's first `puts total` hasn't been reached yet by step 4, so
  // `pre` never rendered and the assertion failed with "element(s) not
  // found", not a real product bug.
  await codePage.openTab('Saída');
  await expect(codePage.output).toContainText('0');

  await codePage.goToStart();
  await codePage.stepForward();

  await codePage.openTab('Variáveis');
  // Ruby starter example (app.ts STARTER_CODE.ruby): `n = 5; total = 0;
  // i = 0; while i < n ... end`. driver.rb's first step event fires
  // before line 1 (`n = 5`) executes, same "before" semantics JDI already
  // has for Java — one step forward from goToStart() lands past that
  // assignment, where `total` is already a real, named local variable in
  // scope (assigned `0` on line 2, visible even before its own line
  // executes per Ruby's lexical local-variable scoping — see driver.rb's
  // module doc comment / tasks.md for that empirical finding).
  await expect(codePage.variables).toContainText('total');
  await expect(codePage.variables).not.toContainText('local_');
});

test('shows a clear error instead of getting stuck when a stale execution_id no longer exists server-side', async ({
  codePage,
}) => {
  // Regression test for the third "Executando…" bug found this session:
  // the app auto-loads whatever execution_id is in localStorage on boot
  // (spec.md "Reconexão"). Since the API's ExecutionStore is in-memory
  // only, any execution_id surviving a container recreation points at
  // nothing — GET /trace 404s, and the fix (ExecutionSessionService.load's
  // error handler) must move status to 'failed', not leave it 'pending'.
  await codePage.setStoredExecutionId('this-id-does-not-exist-anymore');
  await codePage.goto();

  await expect(codePage.runButton).toHaveText('Run', { timeout: 10_000 });
  await expect(codePage.errorMessage).toContainText('execution not found');
});

test('rejects Java code without a class named Main with a clear inline error, not a silent hang', async ({
  codePage,
}) => {
  await codePage.goto();

  // The Monaco editor's content is what gets submitted — replace with code
  // missing the required `class Main` (see ExecutionsResource's
  // server-side validation, api/src/main/java/.../web/ExecutionsResource.java).
  // Uses replaceCode (CodeEditorPage → support/monaco.ts) rather than a raw
  // click+press+type sequence: two separate, empirically-confirmed races
  // in the naive version made this flaky (see that helper's doc comment).
  // Notably, this test's own assertion happened to still pass even while
  // silently corrupted by the first race (the leftover Main.java filename
  // in the compiler error's location prefix satisfies
  // `.toContainText('Main')` regardless of the actual submitted code) —
  // so it was testing the wrong thing until this was found and fixed.
  await codePage.replaceCode('class Solution { void run() {} }');
  await codePage.run();

  await expect(codePage.errorMessage).toContainText('Main', { timeout: 10_000 });
});

test('switches between all five panel tabs', async ({ codePage }) => {
  await codePage.goto();

  const tabs = ['Variáveis', 'Call Stack', 'Saída', 'Complexidade', 'Timeline'];
  for (const tab of tabs) {
    await codePage.openTab(tab);
    await expect(codePage.tab(tab)).toHaveAttribute('aria-selected', 'true');
  }
});

test('blocks Java code that spawns a real thread, with a clear message instead of a silent hang or a false completion', async ({
  codePage,
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
  await codePage.goto();

  // Uses replaceCode (CodeEditorPage → support/monaco.ts): a peer session
  // flagged this test failing deterministically with a javac syntax error
  // instead of exercising the JDI multi-thread check at all. Root-caused
  // (not assumed) with a throwaway debug test dumping `.view-line`
  // contents after the old raw click+press+type sequence: `type`'s
  // per-keystroke handling of this brace/paren/quote-heavy one-liner
  // desynced from the selection, silently leaving 9 stale lines of the
  // starter example below the typed text — so the submitted "file" never
  // compiled. See the helper's doc comment for the second, independent
  // select-all race also found and fixed in the same pass.
  await codePage.replaceCode(
    'class Main { public static void main(String[] a) throws InterruptedException { ' +
      'Thread t = new Thread(() -> System.out.println("hi")); t.start(); t.join(); } }',
  );

  await codePage.runAndWaitForFinish();

  await expect(codePage.banner).toContainText('multi-thread execution is not supported yet', {
    timeout: 10_000,
  });
});

test('shows the real exception instead of silently reporting success when Java code throws uncaught', async ({
  codePage,
}) => {
  // Regression test for a real bug found while validating an unrelated
  // hardening fix (-XX:MaxDirectMemorySize): jdi/Debugger.java's exit-code
  // check only special-cased exit 137 (SIGKILL/OOM) — an uncaught exception
  // in the target (conventionally exit 1) was never checked at all, so the
  // driver fell through to its own normal exit 0. ProcessSandboxRunner/
  // ExecutionJob (API side) only decide completed-vs-failed from the
  // driver's own exit code, never from the trace's event content — so ANY
  // uncaught exception in sandboxed Java code (an extremely common case for
  // a step-through learning tool: users write bugs on purpose) silently
  // reported `status: "completed"` with a truncated trace and zero
  // indication anything failed. Confirmed via POST /executions before
  // fixing: a program that prints "before" then throws an
  // ArrayIndexOutOfBoundsException showed 1 step + "before", status
  // completed — the crash simply vanished. Fixed with a `System.exit(1)`
  // (same pattern the multi-thread block already used) plus capturing the
  // target's own stderr (already redirected to the driver) to surface the
  // real exception message as a clean error event.
  await codePage.goto();

  await codePage.replaceCode(
    'public class Main { public static void main(String[] a) { ' +
      'System.out.println("before"); int[] arr = new int[3]; ' +
      'System.out.println(arr[10]); System.out.println("after"); } }',
  );

  await codePage.runAndWaitForFinish();

  await expect(codePage.banner).toContainText('ArrayIndexOutOfBoundsException', {
    timeout: 10_000,
  });
});
