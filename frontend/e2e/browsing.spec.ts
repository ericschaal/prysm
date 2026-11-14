// Browsing Settings while screen sync runs: the state ticks (about once a
// second) never remount a page or move it, each page starts at the top and
// its back button returns to where the list was, and a page's save line
// speaks only of its own saves.
import { expect, test } from './prysm';

test.use({ viewport: { width: 390, height: 600 } });

/** In the page before it loads: layout shifts and every save line seen. */
function watch() {
  const describe = (node: Node | null | undefined) => {
    if (!node) return '?';
    const element = node instanceof Element ? node : node.parentElement;
    if (!element) return node.nodeName;
    const name = String(element.className || '').split(' ')[0];
    const text = (element.textContent ?? '').trim().slice(0, 24);
    return `${element.tagName.toLowerCase()}${name ? `.${name}` : ''} "${text}"`;
  };
  const rect = (r: DOMRectReadOnly) =>
    `${Math.round(r.x)},${Math.round(r.y)} ${Math.round(r.width)}x${Math.round(r.height)}`;
  const seen = {
    shifts: null as string[] | null,
    marks: [] as string[],
    lines: [] as string[],
  };
  (window as unknown as { seen: typeof seen }).seen = seen;
  try {
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries() as (PerformanceEntry & {
        value: number;
        hadRecentInput: boolean;
        sources: {
          node?: Node;
          previousRect: DOMRectReadOnly;
          currentRect: DOMRectReadOnly;
        }[];
      })[]) {
        const sources = entry.sources
          .map(
            (s) =>
              `${describe(s.node)} ${rect(s.previousRect)} -> ${rect(s.currentRect)}`,
          )
          .join('; ');
        const last = seen.marks.at(-1) ?? 'load';
        seen.shifts?.push(
          `${entry.value.toFixed(5)} at ${Math.round(entry.startTime)} ms (${last}${entry.hadRecentInput ? ', after input' : ''}) ${location.hash}: ${sources}`,
        );
      }
    }).observe({ type: 'layout-shift', buffered: true });
    seen.shifts = [];
  } catch {
    // WebKit has no layout-shift entries.
  }
  const record = () => {
    const line = document.querySelector('.save-status')?.textContent ?? '';
    const text = `${location.hash} ${line}`;
    if (seen.lines.at(-1) !== text) seen.lines.push(text);
    requestAnimationFrame(record);
  };
  requestAnimationFrame(record);
}

type Seen = { shifts: string[] | null; marks: string[]; lines: string[] };
const seen = (page: import('@playwright/test').Page) =>
  page.evaluate(() => (window as unknown as { seen: Seen }).seen);
const mark = (page: import('@playwright/test').Page, step: string) =>
  page.evaluate((step) => {
    (window as unknown as { seen: Seen }).seen.marks.push(
      `${step} @${Math.round(performance.now())}`,
    );
  }, step);

test('Settings stays put under state ticks and pages keep their own place and save line', async ({
  page,
  prysm,
  startSender,
}) => {
  await page.addInitScript(watch);
  const sender = startSender();
  await page.goto('/');
  await mark(page, 'power on');
  await page
    .getByRole('region', { name: 'Your lights' })
    .getByRole('button', { name: 'Power' })
    .click();
  await sender.waitFor('connected');
  await expect
    .poll(async () => (await prysm.state()).session.state)
    .toBe('running');

  await mark(page, 'to Settings');
  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page).toHaveURL(/#\/settings$/);
  // The same nodes after several ticks: nothing remounted.
  const nodes = await page.evaluateHandle(() => [
    document.querySelector('.page-heading h2'),
    document.querySelector('.save-status'),
    document.querySelector('.row-link'),
  ]);
  await page.waitForTimeout(3500);
  expect(
    await nodes.evaluate((before) =>
      before.every(
        (node, index) =>
          node?.isConnected &&
          node ===
            [
              document.querySelector('.page-heading h2'),
              document.querySelector('.save-status'),
              document.querySelector('.row-link'),
            ][index],
      ),
    ),
    'no node of Settings was replaced by a state tick',
  ).toBe(true);

  // A page opens at its top; its back button returns to the list's place.
  await mark(page, 'scroll to bottom');
  await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
  const listPlace = await page.evaluate(() => window.scrollY);
  expect(listPlace).toBeGreaterThan(0);
  await mark(page, 'to About');
  await page.getByRole('button', { name: /^About this Prysm/ }).click();
  await expect(page).toHaveURL(/#\/settings\/about$/);
  expect(await page.evaluate(() => window.scrollY)).toBe(0);
  await mark(page, 'back to Settings');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page).toHaveURL(/#\/settings$/);
  expect(await page.evaluate(() => window.scrollY)).toBe(listPlace);

  // A save started on Settings is not Fine-tuning's: that page says all saved.
  await mark(page, 'scroll to top');
  await page.evaluate(() => window.scrollTo(0, 0));
  await page
    .getByRole('region', { name: 'Picture' })
    .getByRole('checkbox', { name: 'Remove black bars' })
    .click();
  await mark(page, 'to Fine-tuning');
  await page.getByRole('button', { name: /^Fine-tuning/ }).click();
  await expect(page).toHaveURL(/#\/settings\/picture$/);
  await page.waitForTimeout(2500);
  const { lines, shifts } = await seen(page);
  expect(
    lines.filter((line) => line.startsWith('#/settings/picture ')),
  ).toEqual(['#/settings/picture All changes saved']);

  // Nothing moved by itself on the way (Chromium measures layout shift).
  if (shifts !== null) expect(shifts).toEqual([]);
});
