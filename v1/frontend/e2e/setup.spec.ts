// The setup wizard against a real engine: from empty settings to a saved
// setup, written to the settings file and summed up on Home.
import { expect, test } from './prysm';

test.use({ settings: { processing: { brightness_percent: 80 } } });

test.beforeEach(async ({ page }) => {
  // “No lights” is a developer option: on in this browser.
  await page.addInitScript(() =>
    window.localStorage.setItem('prysm.developer', 'on'),
  );
});

test('setup saves a TV source and no lights, on disk and on Home', async ({
  page,
  prysm,
}) => {
  await page.goto('/');
  const wizard = page.getByRole('form', { name: 'Video source' });
  await wizard
    .getByLabel('Capture from')
    .selectOption({ label: 'Capture-only TV (remote)' });
  await page.getByRole('button', { name: 'Next' }).click();

  await page
    .getByRole('form', { name: 'Your lights' })
    .getByLabel('Send colours to')
    .selectOption({ label: 'No lights (test)' });
  await page.getByRole('button', { name: 'Next' }).click();

  const review = page.getByRole('form', { name: 'Review and start' });
  await expect(review).toContainText('Capture-only TV (remote)');
  await expect(review).toContainText('No lights (test)');
  await page.getByRole('button', { name: 'Save only' }).click();

  await expect(
    page.getByRole('status').filter({ hasText: 'Setup saved.' }),
  ).toBeVisible();
  const saved = await prysm.savedSettings();
  expect(saved.capture).toEqual({ kind: 'remote' });
  expect(saved.output).toEqual({ kind: 'null' });

  const setup = page.getByRole('region', { name: 'Your setup' });
  // Device names never break at a hyphen: it is a non-breaking one.
  await expect(setup).toContainText(/Capture.only TV \(remote\)/);
  await expect(setup).toContainText(/320\s×\s180/);
  await expect(setup).toContainText('No lights (test)');
});

test('a step with a missing value says so at its field', async ({ page }) => {
  await page.goto('/');
  const wizard = page.getByRole('form', { name: 'Video source' });
  await expect(wizard.getByLabel('Capture from')).toHaveValue('device');
  await page.getByRole('button', { name: 'Next' }).click();

  const path = wizard.getByLabel('Capture device');
  await expect(path).toBeFocused();
  await expect(path).toHaveAttribute('aria-invalid', 'true');
  await expect(path).toHaveAccessibleDescription('Enter a value.');
  await expect(wizard.getByRole('status')).toHaveText(
    'Check the marked field to continue.',
  );
});
