// What people do with the remote: change settings, read a long log, calibrate
// and leave, and the first run of each package. `press` checks focus after
// every key.
import { expect, test, type Page } from '@playwright/test';
import {
  addLights,
  controls,
  expectFocus,
  expectSizes,
  focusedId,
  lightingTarget,
  openTv,
  press,
  pressUntil,
  publishLog,
  supervisorState,
} from './remote';

const start = { full: 'mode-off', slim: 'primary' } as const;
const settingsTop = { full: 'autostart', slim: 'prysm-address' } as const;
const statusTitle = (page: Page) => page.locator('#status-title');

for (const flavor of ['full', 'slim'] as const) {
  test(`${flavor}: Start with TV flips with OK and applies at once`, async ({
    page,
  }) => {
    await openTv(page, flavor, { focus: start[flavor] });
    await pressUntil(page, 'ArrowRight', settingsTop[flavor]);
    await pressUntil(page, 'ArrowDown', 'autostart');
    const row = page.locator('#autostart');
    await expect(row).toHaveAttribute('aria-checked', 'false');
    await press(page, 'Enter');
    await expect(row).toHaveAttribute('aria-checked', 'true');
    await expect(page.locator('#autostart-value')).toHaveText('On');
    await expect
      .poll(async () => (await supervisorState(page)).preferences.autostart)
      .toBe(true);
    await expect(row).toBeFocused();
    await press(page, 'Enter');
    await expect(row).toHaveAttribute('aria-checked', 'false');
    await expect
      .poll(async () => (await supervisorState(page)).preferences.autostart)
      .toBe(false);
  });

  test(`${flavor}: Down scrolls a long diagnostics log to its end and back`, async ({
    page,
  }) => {
    await openTv(page, flavor, { focus: start[flavor] });
    await publishLog(page, 40);
    await pressUntil(page, 'ArrowRight', settingsTop[flavor]);
    await pressUntil(page, 'ArrowDown', 'service-options');
    await press(page, 'Enter');
    await pressUntil(page, 'ArrowDown', 'diagnostics');
    await press(page, 'Enter');
    const log = page.locator('#diagnostics-text');
    await expect(log).toContainText('(attempt 40)');
    const scroll = () =>
      log.evaluate((el) => ({
        top: el.scrollTop,
        end: el.scrollHeight - el.clientHeight,
      }));
    const first = await scroll();
    expect(first.end, 'the log is longer than its box').toBeGreaterThan(0);
    let top = first.top;
    for (let i = 0; i < 40 && top < first.end - 1; i++) {
      await press(page, 'ArrowDown');
      const now = await scroll();
      expect(now.top, 'each Down scrolls further').toBeGreaterThan(top);
      top = now.top;
    }
    expect(top, 'Down reaches the end').toBeGreaterThanOrEqual(first.end - 1);
    await expect(page.locator('#dialog-back')).toBeFocused();
    for (let i = 0; i < 40 && (await scroll()).top > 0; i++)
      await press(page, 'ArrowUp');
    expect((await scroll()).top, 'Up returns to the top').toBe(0);
  });
}

test('full: a lighting mode applies with OK and the status follows', async ({
  page,
}) => {
  await openTv(page, 'full', { focus: 'mode-off' });
  await pressUntil(page, 'ArrowLeft', 'mode-screen_sync');
  await press(page, 'Enter');
  await expect(statusTitle(page)).toHaveText('Screen sync on');
  await expect(page.locator('#mode-screen_sync')).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  expect(await lightingTarget(page)).toBe('screen_sync');
  await pressUntil(page, 'ArrowRight', 'mode-off');
  await press(page, 'Enter');
  await expect(statusTitle(page)).toHaveText('Lights off');
  expect(await lightingTarget(page)).toBe('off');
});

test('slim: the Prysm address row saves a new address', async ({ page }) => {
  await openTv(page, 'slim', { focus: 'primary' });
  await pressUntil(page, 'ArrowRight', 'prysm-address');
  await press(page, 'Enter');
  const input = page.locator('#prysm-address-input');
  await expect(input).toBeFocused();
  await expect(input).toHaveValue('192.168.1.20:8787');
  // Typing is the TV's on-screen keyboard; Playwright fills the field.
  await input.fill('192.168.1.30');
  await press(page, 'ArrowDown');
  await expect(page.locator('#prysm-address-input-apply')).toBeFocused();
  await press(page, 'Enter');
  await expect(page.locator('#modal')).toHaveCount(0);
  await expect(page.locator('#prysm-address-value')).toHaveText(
    '192.168.1.30:8787',
  );
  await expect(page.locator('#prysm-address')).toBeFocused();
  await expect
    .poll(async () => (await supervisorState(page)).preferences.prysm_address)
    .toBe('http://192.168.1.30:8787');
});

for (const flavor of ['full', 'slim'] as const) {
  test(`${flavor}: Calibrate colours opens the calibration screen; Back leaves it`, async ({
    page,
  }) => {
    await openTv(page, flavor, { focus: start[flavor] });
    if (flavor === 'slim') {
      await press(page, 'Enter');
      await expect(page.locator('#primary')).toHaveText('Stop capture');
    }
    await pressUntil(page, 'ArrowRight', settingsTop[flavor]);
    const row = page.locator('#calibrate');
    await expect(row).toBeEnabled();
    await pressUntil(page, 'ArrowDown', 'calibrate');
    // Nothing is drawn over a pattern: the screen has no control to focus.
    await press(page, 'Enter', { check: false });
    const screen = page.getByRole('region', { name: 'Calibration' });
    await expect(screen.getByRole('heading')).toHaveText('Calibrate colours');
    expect(await controls(page)).toEqual([]);
    expect(await focusedId(page)).toBe('(nothing)');
    await expectSizes(page);
    expect(await lightingTarget(page)).toBe('screen_sync');
    await press(page, 'Back');
    await expect(screen).toHaveCount(0);
    await expect(row).toBeFocused();
    expect(await lightingTarget(page), 'lighting stays as it is').toBe(
      'screen_sync',
    );
  });
}

test('full first run: set up the service, then the lights from the phone', async ({
  page,
}) => {
  await openTv(page, 'full', {
    flags: ['offline', 'nolights'],
    focus: 'primary',
  });
  await expect(statusTitle(page)).toHaveText('Prysm needs setting up');
  await expect(page.locator('#primary')).toHaveText('Set up service');
  await press(page, 'Enter', { check: false });
  await expect(statusTitle(page)).toHaveText('Set up your lights');
  await expectFocus(page);
  await expect(page.locator('.setup-qr')).toBeVisible();
  await expect(page.locator('#portal-address-text')).toHaveText(
    '192.168.1.5:8787',
  );
  await expect(page.locator('.modes button')).toHaveCount(0);
  await expectSizes(page);
  await addLights(page);
  await expect(statusTitle(page)).toHaveText('Lights off');
  await expect(page.locator('#mode-off')).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  await expect(page.locator('.setup-qr')).toHaveCount(0);
  await expect(page.locator('#primary')).toHaveCount(0);
  await expectFocus(page);
  expect(await lightingTarget(page), 'nothing started').toBe('off');
  expect(await supervisorState(page)).toMatchObject({
    flavor: 'full',
    target: 'off',
  });
});

test('slim first run: add the address, then the lights from the phone', async ({
  page,
}) => {
  await openTv(page, 'slim', {
    flags: ['noaddress', 'nolights'],
    focus: 'primary',
  });
  await expect(statusTitle(page)).toHaveText('Set up this TV');
  await expect(page.locator('#primary')).toHaveText('Add Prysm address');
  await press(page, 'Enter');
  const input = page.locator('#prysm-address-input');
  await expect(input).toBeFocused();
  await input.fill('192.168.1.20');
  await press(page, 'ArrowDown');
  await press(page, 'Enter');
  await expect(page.locator('#modal')).toHaveCount(0);
  await expect(statusTitle(page)).toHaveText('Set up your lights');
  await expect(page.locator('.setup-qr')).toBeVisible();
  await expect(page.locator('#portal-address-text')).toHaveText(
    '192.168.1.20:8787',
  );
  await expect(page.locator('.modes button')).toHaveCount(0);
  await expectSizes(page);
  await addLights(page);
  await expect(statusTitle(page)).toHaveText('Capture off');
  await expect(page.locator('#mode-off')).toBeVisible();
  await expect(page.locator('#primary')).toHaveText('Start capture');
  await expectFocus(page);
  expect(await supervisorState(page), 'nothing started').toMatchObject({
    flavor: 'slim',
    enabled: false,
  });
});
