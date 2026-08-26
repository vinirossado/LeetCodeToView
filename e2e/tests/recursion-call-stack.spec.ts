import { expect, test } from './support/fixtures';

// Real-app validation for the recursion/call-stack clarity item (tasks.md):
// this project's own explicitly-cited inspiration, Philip Guo's Python
// Tutor (SIGCSE 2013 paper), treats per-frame stack-variable inspection as
// its core value proposition for teaching recursion. Before this fix, this
// app's Variables panel always showed only the innermost frame's locals,
// no matter which call-stack frame a user looked at or clicked — clicking
// did nothing. This is the JAVA case (jdi/Debugger.java's `frames` array,
// see StepEvent.frames's doc comment) — see
// recursion-call-stack-csharp.spec.ts and recursion-call-stack-ruby.spec.ts
// for the same validation against C#'s stepping.rs and Ruby's driver.rb.
//
// factorial(5), recursing down to the n<=1 base case, gives a real 6-deep
// call stack (factorial x5 + main) with a DISTINCT `n` value per frame —
// exactly the scenario this feature exists for.
const FACTORIAL_JAVA = [
  'public class Main {',
  '    static int factorial(int n) {',
  '        if (n <= 1) {',
  '            return 1;',
  '        }',
  '        return n * factorial(n - 1);',
  '    }',
  '    public static void main(String[] args) {',
  '        int result = factorial(5);',
  '        System.out.println(result);',
  '    }',
  '}',
].join('\n');

test('clicking a call-stack frame shows THAT frame\'s own real local variables in the Variables panel, not always the innermost one', async ({
  codePage,
}) => {
  await codePage.goto();
  await codePage.replaceCode(FACTORIAL_JAVA);

  // Line 4 is the base case (`return 1;`) — hit only once execution has
  // recursed all the way down, i.e. at the DEEPEST point of the call stack.
  await codePage.toggleBreakpointOnLine(4);
  await codePage.runAndWaitForFinish();
  await codePage.goToStart();
  await codePage.nextBreakpoint();

  await codePage.openTab('Call Stack');

  const frameButtons = codePage.callStackFrameButtons();
  // 5 factorial() frames (n=5,4,3,2,1) + main = 6.
  await expect(frameButtons).toHaveCount(6);

  // Sanity: innermost frame (index 0) is the currently-executing one --
  // Variables should already show it by default before any click.
  await codePage.openTab('Variáveis');
  await expect(codePage.variables).toContainText('n');
  await expect(codePage.variables).toContainText('1'); // factorial(1)'s own n

  // Click the frame for factorial(3) (index 2: innermost-first order is
  // n=1, n=2, n=3, n=4, n=5, main) and confirm the Variables panel switches
  // to THAT frame's own, real, distinct local — not the innermost one.
  await codePage.openTab('Call Stack');
  await frameButtons.nth(2).click();
  await expect(frameButtons.nth(2)).toHaveClass(/selected/);

  await codePage.openTab('Variáveis');
  await expect(codePage.frameContextLabel).toBeVisible();
  await expect(codePage.frameContextLabel).toContainText('factorial');
  const dtTexts = await codePage.variables.locator('dt').allTextContents();
  expect(dtTexts).toEqual(['n']);
  const ddTexts = await codePage.variables.locator('dd').allTextContents();
  expect(ddTexts).toEqual(['3']); // factorial(3)'s own n, not factorial(1)'s

  // Click the outermost frame (main) and confirm it shows a DIFFERENT
  // variable entirely (`result`/`args`), proving each frame's locals are
  // genuinely independent, real snapshots — not the same data relabeled.
  await codePage.openTab('Call Stack');
  await frameButtons.nth(5).click();
  await codePage.openTab('Variáveis');
  const mainKeys = await codePage.variables.locator('dt').allTextContents();
  expect(mainKeys).not.toContain('n');
});
