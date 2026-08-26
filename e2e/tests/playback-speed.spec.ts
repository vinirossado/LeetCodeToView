import { expect, test } from './support/fixtures';

// Playback speed control (trace-store.service.ts's `setPlaybackSpeed`,
// class doc decision 1): each speed tier has its own hand-tuned autoplay
// tick interval (1x=700ms, 0.75x=1400ms, 0.5x=4000ms, 0.25x=12000ms — NOT a
// straight multiplier of 700ms, see that file's class doc for why). The
// exact timing math is already covered thoroughly by
// trace-store.service.spec.ts's fake-timer tests; this suite only checks
// the real, wired-up UI in a real browser — the <select> renders, reflects
// state, and a slower speed measurably delays autoplay's first step versus
// the 1x default, without asserting exact millisecond boundaries (real
// browser timers are not that precise, and asserting on them would be
// flaky for no real benefit over the unit tests).
const FIVE_STEP_JAVA = [
  'public class Main {',
  '    public static void main(String[] args) {',
  '        int total = 0;',
  '        for (int i = 0; i < 5; i++) {',
  '            total += i;',
  '        }',
  '        System.out.println(total);',
  '    }',
  '}',
].join('\n');

test('playback speed select defaults to 1x and lists all options', async ({ codePage }) => {
  await codePage.goto();
  await expect(codePage.speedSelect).toHaveValue('1');

  const optionValues = await codePage.speedSelect.locator('option').evaluateAll((opts) =>
    opts.map((o) => (o as HTMLOptionElement).value),
  );
  expect(optionValues).toEqual(['1', '0.75', '0.5', '0.25']);
});

test("choosing a slower speed measurably delays autoplay's first step vs 1x", async ({ codePage }) => {
  await codePage.goto();
  await codePage.replaceCode(FIVE_STEP_JAVA);
  await codePage.runAndWaitForFinish();

  // Wall-clock time from clicking play to the current-line decoration first
  // appearing (goToStart leaves cursor at -1, before any step, where no
  // line is highlighted yet — its FIRST appearance is exactly autoplay's
  // first tick landing). Deliberately black-box: doesn't assume anything
  // about which Java line executes first, just measures real elapsed time
  // until the first visible step, comparable across speeds.
  async function timeUntilFirstStepAppears(): Promise<number> {
    await codePage.goToStart();
    const start = Date.now();
    await codePage.togglePlay();
    await expect(codePage.currentLineHighlight).toHaveCount(1, { timeout: 8_000 });
    const elapsed = Date.now() - start;
    await codePage.togglePlay(); // pause again before the next measurement
    return elapsed;
  }

  const at1x = await timeUntilFirstStepAppears();

  await codePage.setPlaybackSpeed('0.5');
  await expect(codePage.speedSelect).toHaveValue('0.5');
  const at05x = await timeUntilFirstStepAppears();

  // 0.5x's tick (4000ms) vs 1x's (700ms) — a 1.5x margin comfortably proves
  // the speed took effect without asserting exact millisecond ratios, which
  // real browser/CI timer jitter would make flaky. Uses 0.5x rather than the
  // even-slower 0.25x (12000ms) purely to keep this test's real wall-clock
  // runtime reasonable — the per-tier interval values themselves are only
  // asserted precisely in trace-store.service.spec.ts's fake-timer tests.
  expect(at05x).toBeGreaterThan(at1x * 1.5);
});
