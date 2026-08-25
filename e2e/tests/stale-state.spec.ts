import { expect, test } from './support/fixtures';
import { CodeEditorPage } from './support/code-editor-page';

// Real-app validation for the "stale UI state" fixes (tasks.md, grounded in
// NN/g's "Visibility of System Status" heuristic):
//
//  1. Switching languages used to leave the PREVIOUS language's
//     execution_id/trace/complexity result on screen until the next Run
//     click made the mismatch obvious.
//  2. Reloading mid-execution (or opening a shared link) used to keep
//     showing whatever starter example happened to be in the editor next
//     to a reconnected trace with real variable values that code could
//     never have produced.

test('switching language resets stale execution state immediately — no leftover execution_id/trace/panels from the previous language', async ({
  codePage,
  page,
}) => {
  await codePage.goto();
  await codePage.runAndWaitForFinish();

  await expect(codePage.executionId).toBeVisible();
  await expect(codePage.variables).toBeVisible(); // real locals from the Java run

  await codePage.selectLanguage('csharp');

  // The whole execution_id/share block only renders `@if (executionId())`
  // in app.html — its disappearance IS the "reset to clean state" signal.
  await expect(codePage.executionId).toHaveCount(0);
  await codePage.openTab('Call Stack');
  await expect(page.getByText('nenhuma execução em andamento')).toBeVisible();
});

test('reloading (a genuinely fresh page load sharing the same browser storage) after a run restores the REAL submitted language+code — not the starter example next to a mismatched trace', async ({
  codePage,
  page,
  context,
}) => {
  const MARKER = 'distinctive-marker-73f1';
  await codePage.goto();
  await codePage.selectLanguage('ruby');
  await codePage.replaceCode(`puts "${MARKER}"`);
  await codePage.runAndWaitForFinish();
  await codePage.openTab('Saída');
  await expect(codePage.output).toContainText(MARKER);

  // A real reload keeps whatever this browser's localStorage already has
  // (code2complexity.lastExecutionId, persisted by app.ts as soon as the
  // execution_id is known — see app.ts's persist effect). The `codePage`
  // fixture's own page can't be used to simulate this: its clearSession()
  // registers an init script that wipes localStorage on EVERY navigation
  // of THAT page (by design, so each test starts clean — see
  // support/fixtures.ts), which would just erase the very id this test
  // means to reconnect with. A second Page in the SAME browser context
  // shares the same localStorage/cookies (storage is per-context, not
  // per-page) but has no such script registered, so navigating IT to '/'
  // is a faithful stand-in for a real F5 on the original tab.
  const reloadedPage = await context.newPage();
  const reloaded = new CodeEditorPage(reloadedPage);
  await reloaded.goto();

  await expect(reloaded.languageSelect).toHaveValue('ruby');
  const restoredCode = await reloadedPage.locator('.view-line').allTextContents();
  expect(restoredCode.join('\n')).toContain(MARKER);

  await reloadedPage.close();
});
