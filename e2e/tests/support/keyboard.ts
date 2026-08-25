import type { Page } from '@playwright/test';

/**
 * A short, stable-ish description of `document.activeElement` — tag,
 * class, ARIA role, title/aria-label, and a snippet of its own text.
 * Used by the keyboard-trap regression test (see
 * `keyboard-accessibility.spec.ts`) to record which real, distinct
 * elements a sequence of Tab presses actually lands on, without needing a
 * dedicated locator for every single one up front.
 */
export async function focusedElementFingerprint(page: Page): Promise<string> {
  return page.evaluate(() => {
    const el = document.activeElement as HTMLElement | null;
    if (!el || el === document.body) return '(none)';
    const role = el.getAttribute('role') ?? '';
    const title = el.getAttribute('title') ?? el.getAttribute('aria-label') ?? '';
    const text = (el.textContent ?? '').trim().slice(0, 24);
    return `${el.tagName}|${el.className}|${role}|${title}|${text}`;
  });
}

/**
 * Presses Tab `count` times, recording a fingerprint of whatever ends up
 * focused after each press (see {@link focusedElementFingerprint}).
 * Returns every fingerprint visited, IN ORDER — a keyboard trap shows up
 * as the same one or two fingerprints repeating for the rest of the
 * sequence instead of new page controls being reached.
 */
export async function tabThrough(page: Page, count: number): Promise<string[]> {
  const visited: string[] = [];
  for (let i = 0; i < count; i++) {
    await page.keyboard.press('Tab');
    visited.push(await focusedElementFingerprint(page));
  }
  return visited;
}
