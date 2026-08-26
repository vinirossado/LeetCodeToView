import { expect, test } from './support/fixtures';

// Ruby counterpart of recursion-call-stack.spec.ts (see that file's own doc
// comment for the full Python-Tutor-inspired rationale) — validates that
// the same per-frame click-to-inspect UX, already shipped for Java and C#,
// now also works for Ruby (tasks.md's per-frame-locals backlog item):
// sandbox/ruby/driver.rb now emits a `frames` array (name+locals per frame,
// innermost-first) per step event, same shape jdi/Debugger.java/stepping.rs
// already produce. The frontend needed ZERO changes — call-stack-panel.
// component.ts/variables-panel.component.ts already render `frames` when
// present, regardless of which language's driver produced it.
//
// Unlike C#, Ruby's TracePoint gives real parameter names for free (`n`),
// so this doesn't need C#'s "assign the parameter into an explicit local"
// workaround — `n` itself is a real, PDB-free, TracePoint-resolved local.
const FACTORIAL_RUBY = [
  'def factorial(n)',
  '  current = n',
  '  if current <= 1',
  '    return 1',
  '  end',
  '  current * factorial(current - 1)',
  'end',
  '',
  'result = factorial(5)',
  'puts result',
].join('\n');

test('Ruby: clicking a call-stack frame shows THAT frame\'s own real local variables in the Variables panel, not always the innermost one', async ({
  codePage,
}) => {
  await codePage.goto();
  await codePage.selectLanguage('ruby');
  await codePage.replaceCode(FACTORIAL_RUBY);

  // Line 4 is the base case (`return 1`) — hit only once execution has
  // recursed all the way down, i.e. at the DEEPEST point of the call stack.
  await codePage.toggleBreakpointOnLine(4);
  await codePage.runAndWaitForFinish();
  await codePage.goToStart();
  await codePage.nextBreakpoint();

  await codePage.openTab('Call Stack');

  const frameButtons = codePage.callStackFrameButtons();
  // 5 factorial() frames (n=5,4,3,2,1) + <main> = 6.
  await expect(frameButtons).toHaveCount(6);

  // Sanity: innermost frame (index 0) is the currently-executing one --
  // Variables should already show it by default before any click.
  await codePage.openTab('Variáveis');
  await expect(codePage.variables).toContainText('n');
  await expect(codePage.variables).toContainText('1'); // factorial(1)'s own n/current

  // Click the frame for factorial(n=3) (index 2: innermost-first order is
  // n=1,2,3,4,5, then <main>) and confirm the Variables panel switches to
  // THAT frame's own, real, distinct locals -- not the innermost one.
  await codePage.openTab('Call Stack');
  await frameButtons.nth(2).click();
  await expect(frameButtons.nth(2)).toHaveClass(/selected/);

  await codePage.openTab('Variáveis');
  await expect(codePage.frameContextLabel).toBeVisible();
  await expect(codePage.frameContextLabel).toContainText('factorial');
  const dtTexts = await codePage.variables.locator('dt').allTextContents();
  expect(dtTexts).toEqual(['n', 'current']);
  const ddTexts = await codePage.variables.locator('dd').allTextContents();
  expect(ddTexts).toEqual(['3', '3']); // factorial(3)'s own n/current, not factorial(1)'s

  // Click the outermost frame (<main>) and confirm it shows a DIFFERENT
  // variable entirely (`result`), proving each frame's locals are
  // genuinely independent, real snapshots -- not the same data relabeled.
  await codePage.openTab('Call Stack');
  await frameButtons.nth(5).click();
  await codePage.openTab('Variáveis');
  await expect(codePage.frameContextLabel).toContainText('<main>');
  const mainKeys = await codePage.variables.locator('dt').allTextContents();
  expect(mainKeys).not.toContain('n');
  expect(mainKeys).toContain('result');
});
