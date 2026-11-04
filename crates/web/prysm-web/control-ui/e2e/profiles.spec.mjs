import { test, expect } from '@playwright/test';
import { calibrationFixture } from './fixture.mjs';

test('profile picker and staged profile management', async ({
  page,
}, testInfo) => {
  const fixture = calibrationFixture();
  const settings = fixture.settings;
  const profiles = settings.settings.calibration;
  profiles.profiles.push({ ...profiles.profiles[0], id: 'game', name: 'Game' });
  let writes = 0;
  await page.route('**/api/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === '/api/settings') {
      if (request.method() === 'PUT') {
        const body = request.postDataJSON();
        expect(body.expected_revision).toBe(settings.revision);
        writes++;
        settings.revision = String(Number(settings.revision) + 1);
        settings.settings = body.settings;
      }
      await route.fulfill({ json: settings });
      return;
    }
    const reply = fixture.respond(
      path,
      request.method(),
      request.postDataJSON() ?? {},
    );
    if (path === '/api/state') {
      if (request.method() === 'PUT') {
        settings.settings.source = request.postDataJSON().source;
      } else {
        reply.json.config_revision = settings.revision;
        reply.json.applied_revision = settings.revision;
        reply.json.calibration_active = false;
        reply.json.source = settings.settings.source;
      }
    }
    await route.fulfill({ json: reply.json });
  });
  await page.goto('/#/overview');
  const picker = page.getByRole('button', { name: /^Color profile/ });
  const option = (name) => page.getByRole('option', { name, exact: true });
  await expect(picker).toContainText('Movie night');
  await picker.click();
  await expect(option('Movie night')).toBeFocused();
  await expect(option('Movie night')).toHaveAttribute('aria-selected', 'true');
  await page.locator('.power-controls').screenshot({
    path: testInfo.outputPath('profiles-dropdown.png'),
    animations: 'disabled',
  });
  await page.keyboard.press('End');
  await expect(option('No correction')).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(picker).toBeFocused();
  expect(writes).toBe(0);
  await picker.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await expect(option('Game')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect.poll(() => settings.settings.calibration.active).toBe('game');
  await page.locator('.power-controls').screenshot({
    path: testInfo.outputPath('profiles-overview.png'),
    animations: 'disabled',
  });
  await page.getByRole('button', { name: 'WLED effects' }).click();
  await expect(picker).toHaveCount(0);
  await page.getByRole('button', { name: 'Screen sync' }).click();
  await expect(picker).toContainText('Game');
  await page
    .getByRole('link', { name: 'Settings', exact: true })
    .first()
    .click();
  const panel = page.getByRole('region', { name: 'Color profiles' });
  const action = (name) =>
    panel.getByRole('button', { name: 'Actions for ' + name });
  const save = page.getByRole('button', { name: 'Save & apply' });
  await action('Game').click();
  await panel.getByRole('button', { name: 'Rename', exact: true }).click();
  const input = panel.getByRole('textbox', { name: 'Profile name' });
  const done = panel.getByRole('button', { name: 'Done', exact: true });
  async function expectAligned() {
    const field = await input.boundingBox();
    const action = await done.boundingBox();
    expect(action.x).toBeGreaterThanOrEqual(field.x + field.width);
    expect(
      Math.abs(action.y + action.height / 2 - field.y - field.height / 2),
    ).toBeLessThan(1);
  }
  await expect(input).toBeFocused();
  await expectAligned();
  await input.fill(' ');
  await save.click();
  await expect(input).toBeFocused();
  await expect(input).toHaveAttribute('aria-invalid', 'true');
  await expectAligned();
  await input.fill('Console');
  await input.press('Enter');
  await action('Console').click();
  await panel.screenshot({
    path: testInfo.outputPath('profiles-actions.png'),
    animations: 'disabled',
  });
  await panel.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(panel.getByText(/will use No correction/)).toBeVisible();
  await panel.getByRole('button', { name: 'Undo' }).click();
  await expect(action('Console')).toBeFocused();
  await expect(panel.getByText('Active', { exact: true })).toBeVisible();
  expect(writes).toBe(1);
  await page.getByRole('button', { name: 'Discard', exact: true }).click();
  await expect(action('Game')).toBeVisible();
  await action('Game').click();
  await panel.getByRole('button', { name: 'Delete', exact: true }).click();
  await action('Movie night').click();
  await panel.getByRole('button', { name: 'Rename', exact: true }).click();
  await input.fill('Cinema');
  await save.click();
  await expect.poll(() => settings.settings.calibration.active).toBeNull();
  expect(settings.settings.calibration.profiles.map((p) => p.name)).toEqual([
    'Cinema',
  ]);
  await expect(panel.getByRole('button', { name: 'Undo' })).toHaveCount(0);
  await page
    .getByRole('link', { name: 'Overview', exact: true })
    .first()
    .click();
  await expect(picker).toContainText('No correction');
  await picker.click();
  await option('Cinema').click();
  await expect.poll(() => settings.settings.calibration.active).toBe('saved');
  await picker.click();
  await option('No correction').click();
  await expect.poll(() => settings.settings.calibration.active).toBeNull();
});
