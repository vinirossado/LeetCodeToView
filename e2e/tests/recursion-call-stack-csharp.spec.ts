import { expect, test } from './support/fixtures';

// C# counterpart of recursion-call-stack.spec.ts (see that file's own doc
// comment for the full Python-Tutor-inspired rationale) — validates that
// the same per-frame click-to-inspect UX, already shipped for Java, now
// also works for C# (tasks.md's per-frame-locals backlog item): sandbox/src/
// com/callback/stepping.rs's `walk_call_stack` now emits a `frames` array
// (name+locals per frame, innermost-first) per step event for C#, same
// shape jdi/Debugger.java already produces. The frontend needed ZERO
// changes — call-stack-panel.component.ts/variables-panel.component.ts
// already render `frames` when present, regardless of which language's
// driver produced it.
//
// Uses an explicit local variable (`current`), not just the `n` PARAMETER,
// because C#'s locals extraction (extract_locals in stepping.rs) only
// resolves true locals via ICorDebugILFrame::GetLocalVariable — method
// PARAMETERS need a separate GetArgument call C# doesn't make (a real,
// pre-existing, unrelated gap confirmed by A/B testing this exact scenario
// against pre-per-frame `main`: identical `local_0`/`local_1` placeholder
// names either way). Assigning the parameter into a real local up front
// sidesteps that gap without masking it, and gives each recursion depth a
// PDB-resolved, distinctly-named, distinctly-valued local to assert on.
const FACTORIAL_CSHARP = [
  'using System;',
  '',
  'class Program {',
  '    static int Factorial(int n) {',
  '        int current = n;',
  '        if (current <= 1) {',
  '            return 1;',
  '        }',
  '        return current * Factorial(current - 1);',
  '    }',
  '    static void Main() {',
  '        int result = Factorial(5);',
  '        Console.WriteLine(result);',
  '    }',
  '}',
].join('\n');

test('C#: clicking a call-stack frame shows THAT frame\'s own real local variables in the Variables panel, not always the innermost one', async ({
  codePage,
}) => {
  await codePage.goto();
  await codePage.selectLanguage('csharp');
  await codePage.replaceCode(FACTORIAL_CSHARP);

  // Line 7 is the base case (`return 1;`) — hit only once execution has
  // recursed all the way down, i.e. at the DEEPEST point of the call stack.
  await codePage.toggleBreakpointOnLine(7);
  await codePage.runAndWaitForFinish();
  await codePage.goToStart();
  await codePage.nextBreakpoint();

  await codePage.openTab('Call Stack');

  const frameButtons = codePage.callStackFrameButtons();
  // 5 Factorial() frames (current=5,4,3,2,1) + Main = 6.
  await expect(frameButtons).toHaveCount(6);

  // Sanity: innermost frame (index 0) is the currently-executing one --
  // Variables should already show it by default before any click.
  await codePage.openTab('Variáveis');
  await expect(codePage.variables).toContainText('current');
  await expect(codePage.variables).toContainText('1'); // Factorial(1)'s own `current`

  // Click the frame for Factorial(current=3) (index 2: innermost-first
  // order is current=1,2,3,4,5, then Main) and confirm the Variables panel
  // switches to THAT frame's own, real, distinct local -- not the
  // innermost one.
  await codePage.openTab('Call Stack');
  await frameButtons.nth(2).click();
  await expect(frameButtons.nth(2)).toHaveClass(/selected/);

  await codePage.openTab('Variáveis');
  await expect(codePage.frameContextLabel).toBeVisible();
  await expect(codePage.frameContextLabel).toContainText('Factorial');
  const dtTexts = await codePage.variables.locator('dt').allTextContents();
  expect(dtTexts).toContain('current');
  const currentIndex = dtTexts.indexOf('current');
  const ddTexts = await codePage.variables.locator('dd').allTextContents();
  expect(ddTexts[currentIndex]).toBe('3'); // Factorial(3)'s own current, not Factorial(1)'s

  // Click the outermost frame (Main) and confirm it shows a DIFFERENT
  // variable entirely (`result`), proving each frame's locals are
  // genuinely independent, real snapshots -- not the same data relabeled.
  await codePage.openTab('Call Stack');
  await frameButtons.nth(5).click();
  await codePage.openTab('Variáveis');
  await expect(codePage.frameContextLabel).toContainText('Main');
  const mainKeys = await codePage.variables.locator('dt').allTextContents();
  expect(mainKeys).not.toContain('current');
  expect(mainKeys).toContain('result');
});
