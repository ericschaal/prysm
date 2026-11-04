import { test, expect } from '@playwright/test';
import { calibrationFixture } from './fixture.mjs';

test('phone flow: no dashboard flash, paired intensity, optional hues, failed save, reload and success', async ({
  page,
}, testInfo) => {
  const fixture = calibrationFixture();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  let releaseSettings;
  const settingsReady = new Promise((resolve) => {
    releaseSettings = resolve;
  });
  await page.route('**/api/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === '/api/settings') await settingsReady;
    const result = fixture.respond(
      path,
      request.method(),
      request.postDataJSON() ?? {},
    );
    await route.fulfill({ status: result.status ?? 200, json: result.json });
  });
  await page.goto('/#/calibrate/test');
  await expect(
    page.getByRole('heading', { name: 'Connecting to your TV…' }),
  ).toBeVisible();
  await expect(
    page.getByRole('navigation', { name: 'Main navigation' }),
  ).toHaveCount(0);
  releaseSettings();
  await page.getByRole('button', { name: 'New profile', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Next', exact: true }),
  ).toBeDisabled();
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await expect(
    page.getByRole('heading', { name: 'Find a natural white' }),
  ).toBeFocused();
  await page.getByRole('slider', { name: 'Temperature' }).fill('35');
  await page.getByRole('slider', { name: 'Tint', exact: true }).fill('-18');
  await expect.poll(() => fixture.view.draft.correction.warmth).toBe(35);
  await page.screenshot({
    path: testInfo.outputPath('white-balance.png'),
    fullPage: true,
  });
  const compare = page.getByRole('button', { name: 'Hold for uncorrected' });
  await compare.focus();
  await page.keyboard.down('Space');
  await expect.poll(() => fixture.view.compare).toBe(true);
  await page.keyboard.up('Space');
  await expect.poll(() => fixture.view.compare).toBe(false);
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await expect(
    page.getByText('Moderate blue on the left · Vivid blue on the right', {
      exact: true,
    }),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Green', exact: true }).click();
  await page.getByRole('slider', { name: 'Color intensity' }).fill('-24');
  await expect.poll(() => fixture.view.draft.correction.intensity).toBe(-24);
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await expect(
    page.getByRole('heading', { name: 'Fine-tune individual hues' }),
  ).toBeFocused();
  await expect(page.getByRole('slider', { name: 'Blue hue' })).toBeVisible();
  await page.getByRole('button', { name: 'Skip hue adjustment' }).click();
  await expect(
    page.getByRole('heading', { name: 'Check colors together' }),
  ).toBeFocused();
  await page.getByRole('button', { name: 'Back', exact: true }).click();
  const picker = page.getByRole('group', { name: 'TV color' });
  await expect(picker.getByRole('button')).toHaveCount(6);
  await expect(
    picker.getByRole('button', { name: 'Blue', exact: true }),
  ).toHaveAttribute('aria-pressed', 'true');
  await page.getByRole('button', { name: 'Cycle colors' }).click();
  await expect.poll(() => fixture.view.paused).toBe(false);
  await expect(page.getByRole('slider', { name: 'Blue hue' })).toHaveCount(0);
  await picker.getByRole('button', { name: 'Cyan', exact: true }).click();
  await expect.poll(() => fixture.view.shade).toBe(3);
  expect(fixture.view.paused).toBe(true);
  await page.getByRole('slider', { name: 'Cyan hue' }).fill('32');
  await expect
    .poll(() => fixture.view.draft.correction.hue)
    .toEqual([0, 0, 0, 32, 0, 0]);
  await picker.getByRole('button', { name: 'Red', exact: true }).click();
  await expect(page.getByRole('slider', { name: 'Red hue' })).toHaveValue('0');
  await picker.getByRole('button', { name: 'Cyan', exact: true }).click();
  await expect(page.getByRole('slider', { name: 'Cyan hue' })).toHaveValue(
    '32',
  );
  await expect(
    picker.getByRole('button', { name: 'Cyan', exact: true }),
  ).toHaveAttribute('aria-pressed', 'true');
  await page.screenshot({
    path: testInfo.outputPath('colors.png'),
    fullPage: true,
  });
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await page.getByRole('button', { name: 'Dusk', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Amber on the left');
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await page.getByRole('textbox', { name: 'Profile name' }).fill('Living room');
  fixture.failSave(true);
  await page.getByRole('button', { name: 'Save and use' }).click();
  await expect(page.getByRole('alert')).toContainText('Could not save');
  expect(fixture.view.ended).toBeNull();
  fixture.failSave(false);
  await page.reload();
  await expect(
    page.getByRole('heading', { name: 'Make it yours' }),
  ).toBeVisible();
  await page.getByRole('textbox', { name: 'Profile name' }).fill('Living room');
  await page.getByRole('button', { name: 'Save and use' }).click();
  await expect(
    page.getByRole('heading', { name: 'Your light. Your color.' }),
  ).toBeVisible();
  expect(fixture.settings.settings.calibration.profiles[0].name).toBe(
    'Movie night',
  );
  expect(fixture.view.draft.correction).toEqual({
    intensity: -24,
    hue: [0, 0, 0, 32, 0, 0],
    warmth: 35,
    tint: -18,
    midtones: 0,
  });
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          globalThis.document.documentElement.scrollWidth <=
          globalThis.innerWidth,
      ),
    )
    .toBe(true);
  await page.screenshot({
    path: testInfo.outputPath('saved.png'),
    fullPage: true,
  });
  await page.getByRole('link', { name: 'Return to Overview' }).click();
  await expect(
    page.getByRole('navigation', { name: 'Main navigation' }),
  ).toBeVisible();
  await page
    .getByRole('link', { name: 'Settings', exact: true })
    .first()
    .click();
  await expect(page).toHaveURL(/#\/settings$/);
  await page.goBack();
  await expect(page).toHaveURL(/#\/overview$/);
  expect(errors).toEqual([]);
});

test('Stop lighting is available before confirmation', async ({ page }) => {
  const fixture = calibrationFixture();
  await page.route('**/api/**', async (route) => {
    const request = route.request();
    const result = fixture.respond(
      new URL(request.url()).pathname,
      request.method(),
      request.postDataJSON() ?? {},
    );
    await route.fulfill({ status: result.status ?? 200, json: result.json });
  });
  await page.goto('/#/calibrate/test-stop');
  await page.getByRole('button', { name: 'New profile', exact: true }).click();
  await page
    .getByRole('button', { name: 'Stop lighting', exact: true })
    .click();
  await expect(
    page.getByRole('heading', { name: 'Calibration ended' }),
  ).toBeVisible();
  expect(fixture.view.ended).toBe('Lighting stopped.');
  await expect(page.getByRole('alert')).toHaveCount(0);
});
