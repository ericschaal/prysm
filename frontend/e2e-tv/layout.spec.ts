// Nothing on the home screen moves: while Start with TV is saved, while the
// first state arrives on a cold start, and between normal and failure states.
// Each test compares the layout boxes of the screen's main slots.
import { expect, test, type Page } from '@playwright/test';
import {
  demoUrl,
  expectFocus,
  layoutBoxes,
  openTv,
  press,
  pressUntil,
  settled,
  type Flavor,
} from './remote';

const start = { full: 'mode-off', slim: 'primary' } as const;
const settingsTop = { full: 'autostart', slim: 'instance' } as const;
const slots = {
  full: [
    '.overview',
    '.lighting',
    '#portal-address',
    '.settings',
    '#autostart',
    '#calibrate',
    '#service-options',
  ],
  slim: [
    '.overview',
    '#primary',
    '.lighting',
    '#portal-address',
    '.settings',
    '#instance',
    '#autostart',
    '#calibrate',
    '#service-options',
  ],
};

/** The slots' boxes, each of which must be on the screen. */
async function measure(page: Page, flavor: Flavor) {
  await settled(page);
  const boxes = await layoutBoxes(page, slots[flavor]);
  for (const [selector, box] of Object.entries(boxes))
    expect(
      box && box.width > 0 && box.height > 0,
      `${selector} is laid out`,
    ).toBe(true);
  return boxes;
}

for (const flavor of ['full', 'slim'] as const) {
  test(`${flavor}: saving Start with TV moves nothing, the lights card included`, async ({
    page,
  }) => {
    await openTv(page, flavor, { focus: start[flavor] });
    const before = await measure(page, flavor);
    await pressUntil(page, 'ArrowRight', settingsTop[flavor]);
    await pressUntil(page, 'ArrowDown', 'autostart');
    expect(await measure(page, flavor), 'with the row focused').toEqual(before);
    // While the supervisor has not answered the save yet.
    await page.evaluate(() => window.demo.supervisor.hold());
    await press(page, 'Enter');
    expect(await measure(page, flavor), 'while it saves').toEqual(before);
    await page.evaluate(() => window.demo.supervisor.release());
    await expect(page.locator('#autostart-value')).toHaveText('On');
    expect(await measure(page, flavor), 'once saved').toEqual(before);
  });

  test(`${flavor}: on a cold start nothing moves when the first state arrives`, async ({
    page,
  }) => {
    await page.goto(demoUrl(flavor, ['hold']));
    await expect(page.locator('#status-title')).toHaveText('Connecting…');
    const before = await measure(page, flavor);
    await page.evaluate(() => window.demo.supervisor.release());
    await expect(page.locator(`#${start[flavor]}`)).toBeFocused();
    await expect(page.locator('#status-title')).not.toHaveText('Connecting…');
    await expect(page.locator('#portal-address-text')).not.toHaveText(' ');
    await expectFocus(page);
    expect(await measure(page, flavor), 'with the state').toEqual(before);
  });

  test(`${flavor}: WLED failing moves neither the cards nor the buttons`, async ({
    page,
  }) => {
    await lightsOn(page, flavor);
    const normal = await measure(page, flavor);
    // WLED unplugged: the portal reports a failed output.
    await page.evaluate(() =>
      window.demo.application.publish({
        failure: {
          kind: 'adapter',
          section: 'output',
          message: 'WLED at 192.168.1.50 did not answer',
        },
        session: { state: 'stopped' },
      }),
    );
    await expect(page.locator('.lighting')).toHaveClass(/attention/);
    expect(await measure(page, flavor), 'with an output failure').toEqual(
      normal,
    );
  });

  test(`${flavor}: capture failing on the TV moves neither the cards nor the buttons`, async ({
    page,
  }) => {
    await lightsOn(page, flavor);
    const normal = await measure(page, flavor);
    await page.evaluate(() =>
      window.demo.supervisor.publish({
        failure: { kind: 'other', message: 'Capture stopped: no video frames' },
        capturing: false,
        fps: null,
      }),
    );
    await expect(page.locator('.overview')).toHaveClass(/attention/);
    expect(await measure(page, flavor), 'with a capture failure').toEqual(
      normal,
    );
  });
}

/** Screen sync on (full), or capture on (slim): the normal running state. */
async function lightsOn(page: Page, flavor: Flavor) {
  await openTv(page, flavor, { focus: start[flavor] });
  if (flavor === 'full') {
    await pressUntil(page, 'ArrowLeft', 'mode-screen_sync');
    await press(page, 'Enter');
    await expect(page.locator('#status-title')).toHaveText('Screen sync on');
  } else {
    await press(page, 'Enter');
    await expect(page.locator('#primary')).toHaveText('Stop capture');
  }
}
