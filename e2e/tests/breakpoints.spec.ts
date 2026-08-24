import { expect, test } from './support/fixtures';

// Investigates a user report: "breakpoint doesn't seem to work" — the
// gutter dot appears, but 'Próximo breakpoint' runs straight to the end of
// the trace instead of stopping. Hypothesis: sandbox/jdi/Debugger.java uses
// StepRequest.STEP_LINE, which only fires when execution reaches a line that
// actually has a bytecode line-table entry — a breakpoint set on a
// non-statement line (blank line, comment, a bare closing brace) can never
// match any recorded step.line, so trace-store.service.ts's
// runToNextBreakpoint() correctly falls through to jumpToEnd() every time.
// These tests set a breakpoint on the Java starter example (app.ts
// STARTER_CODE.java):
//   1  public class Main {
//   2      public static void main(String[] args) {
//   3          int n = 5;
//   4          int total = 0;
//   5          for (int i = 0; i < n; i++) {
//   6              total += i;
//   7              System.out.println(total);
//   8          }
//   9      }
//  10  }

test('a breakpoint on an executable statement line actually stops the cursor there', async ({ codePage }) => {
  await codePage.goto();

  await codePage.toggleBreakpointOnLine(7); // System.out.println(total); — runs 5 times
  await expect(codePage.breakpointGlyphs).toHaveCount(1);

  await codePage.runAndWaitForFinish();

  await codePage.goToStart();
  await codePage.nextBreakpoint();

  // If it actually stopped at the first hit of line 7 (well before the end
  // of a 5-iteration loop's trace), stepping forward and jumping to the end
  // must still be possible.
  await codePage.expectStepForwardEnabled();
  await codePage.expectGoToEndEnabled();

  // Actual breakpoint hit is rendered in red (current-line-breakpoint-hit),
  // not the orange used for plain step-by-step navigation.
  await expect(codePage.currentLineBreakpointHit).toHaveCount(1);
  await expect(codePage.currentLineHighlight).toHaveCount(0);
});

test('clicking Run with a breakpoint already set stops the live-follow cursor there, no manual navigation needed', async ({
  codePage,
}) => {
  await codePage.goto();

  await codePage.toggleBreakpointOnLine(7); // System.out.println(total); — runs 5 times
  await expect(codePage.breakpointGlyphs).toHaveCount(1);

  await codePage.runAndWaitForFinish();

  // The sandbox already ran the whole program server-side (the Run button
  // is back to "Run"), but the client-side live-follow cursor must have
  // stopped at the FIRST hit of line 7 instead of racing to the trace's
  // tip — this is the actual product requirement: it "feels" like a real
  // breakpoint even though it's trace-and-replay under the hood.
  await codePage.expectStepForwardEnabled();
  await codePage.expectGoToEndEnabled();
  await expect(codePage.currentLineBreakpointHit).toHaveCount(1);
});

test('a breakpoint on a non-statement line (closing brace) is silently never hit', async ({ codePage }) => {
  await codePage.goto();

  await codePage.toggleBreakpointOnLine(8); // the for-loop's closing `}` — no line-table entry
  await expect(codePage.breakpointGlyphs).toHaveCount(1);

  await codePage.runAndWaitForFinish();

  await codePage.goToStart();
  await codePage.nextBreakpoint();

  // runToNextBreakpoint() never finds a step whose line === 8, so it falls
  // through to jumpToEnd() — both "one more step" and "jump to end" become
  // disabled because the cursor is already at the trace's pseudo-end.
  await codePage.expectStepForwardDisabled();
  await codePage.expectGoToEndDisabled();

  // No red "actually stopped here" highlight — it lands on the pseudo-end
  // via the ordinary (orange) decoration, giving the user a visible signal
  // that the breakpoint itself was never reached.
  await expect(codePage.currentLineBreakpointHit).toHaveCount(0);
});
