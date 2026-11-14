// Prysm restarting under an open page: the page reconnects by itself, and
// once the restart brought another build it says so and offers a reload.
import { expect, test } from './prysm';

/** The page's own script, named by its content. */
const script = /\/assets\/index-[\w-]+\.js/;

test('the page reconnects after a restart, and offers a reload after an update', async ({
  page,
  prysm,
}) => {
  await page.goto('/');
  const connection = page.locator('.connection');
  const updated = page.getByText('Prysm was updated.');
  await expect(connection).toHaveText('Connected');

  // A restart of the same build: back by itself, nothing to reload.
  await prysm.restart();
  await expect(connection).toHaveText('Connected');
  await expect(updated).toBeHidden();

  // An update: the script this page runs is no longer served.
  await page.route(script, (route) => route.fulfill({ status: 404 }));
  await prysm.restart();
  await expect(updated).toBeVisible();
  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(updated).toBeVisible();

  // Reload brings the page Prysm serves now.
  await page.unroute(script);
  await page.getByRole('button', { name: 'Reload' }).click();
  await expect(connection).toHaveText('Connected');
  await expect(updated).toBeHidden();
});
