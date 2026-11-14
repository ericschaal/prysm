// A theme chosen in this browser is in place before the page is drawn: the
// other theme never flashes first.
import { expect, test } from './prysm';

/** The page's own script, which draws it. */
const script = /\/assets\/index-[\w-]+\.js/;

test.use({ colorScheme: 'light' });

test('the chosen theme is applied before the page is drawn', async ({
  page,
}) => {
  await page.goto('/');
  await page.evaluate(() => localStorage.setItem('prysm.theme', 'dark'));

  // Hold the bundle back: whatever is set now was set before any drawing.
  let release = () => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(script, async (route) => {
    await held;
    await route.continue();
  });
  await page.reload({ waitUntil: 'commit' });
  const root = page.locator('html');
  await expect(root).toHaveAttribute('data-theme', 'dark');
  await expect(page.locator('#root')).toBeEmpty();
  expect(
    await root.evaluate((element) => getComputedStyle(element).colorScheme),
  ).toBe('dark');

  release();
  await expect(page.locator('.connection')).toHaveText('Connected');
  await expect(root).toHaveAttribute('data-theme', 'dark');
});
