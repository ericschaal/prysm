// On every screen and dialog of both packages: all controls are reachable
// with arrows alone, focus stays visible and inside the safe area after each
// arrow (checked by `press`), and controls and text are large enough.
import { expect, test, type Page } from '@playwright/test';
import {
  expectAllReachable,
  expectFocus,
  expectSizes,
  openTv,
  press,
  pressUntil,
  settled,
  startScreenSync,
  type Flavor,
} from './remote';

interface Screen {
  flavor: Flavor;
  name: string;
  flags?: string[];
  /** Where focus rests when the page opens. */
  focus: string;
  /** Keys from there to the screen or dialog. */
  reach?: (page: Page, flavor: Flavor) => Promise<void>;
}

/** To the settings column, then down to Service options, and OK. */
async function serviceOptions(page: Page, flavor: Flavor) {
  await pressUntil(
    page,
    'ArrowRight',
    flavor === 'full' ? 'autostart' : 'prysm-address',
  );
  await pressUntil(page, 'ArrowDown', 'service-options');
  await press(page, 'Enter');
  await expect(page.locator('#dialog-title')).toHaveText('Service options');
}
const choose =
  (id: string, title: string) => async (page: Page, flavor: Flavor) => {
    await serviceOptions(page, flavor);
    await pressUntil(page, 'ArrowDown', id);
    await press(page, 'Enter');
    await expect(page.locator('#dialog-title')).toHaveText(title);
  };
const closeDialog = async (page: Page) => {
  await press(page, 'Back');
  await expect(page.locator('#dialog-title')).toHaveText('Close Prysm?');
};
const dialogs = (flavor: Flavor, focus: string): Screen[] => [
  { flavor, focus, name: 'Service options', reach: serviceOptions },
  {
    flavor,
    focus,
    name: 'Diagnostics',
    reach: choose('diagnostics', 'Diagnostics'),
  },
  {
    flavor,
    focus,
    name: 'Remove service setup',
    reach: choose('cleanup', 'Remove service setup?'),
  },
  { flavor, focus, name: 'Close Prysm', reach: closeDialog },
];

const screens: Screen[] = [
  { flavor: 'full', name: 'Home, lights off', focus: 'light-power' },
  {
    flavor: 'full',
    name: 'Home, screen sync on',
    focus: 'light-power',
    reach: async (page) => {
      await startScreenSync(page);
    },
  },
  {
    flavor: 'full',
    name: 'Home, blocked capture with Power off',
    focus: 'light-power',
    reach: async (page) => {
      await page.evaluate(() =>
        window.demo.application.publish({
          target: 'off',
          session: {
            state: 'stopped',
            failure: {
              kind: 'adapter',
              section: 'capture',
              message: 'Capture stopped: no video frames',
            },
          },
        }),
      );
      await expect(page.locator('#status-title')).toHaveText('Lights off');
      await expect(page.locator('#lighting-status')).toContainText(
        'The video source isn’t working.',
      );
      await expect(page.locator('#light-power')).toHaveAttribute(
        'aria-pressed',
        'false',
      );
      await expect(page.locator('#light-power')).toBeEnabled();
      await expect(
        page.getByRole('group', { name: 'Lighting mode' }).getByRole('button'),
      ).toHaveCount(1);
      await settled(page);
      await expectFocus(page);
    },
  },
  {
    flavor: 'full',
    name: 'Home, service not set up',
    flags: ['offline'],
    focus: 'primary',
  },
  {
    flavor: 'full',
    name: 'Home, no lights yet',
    flags: ['nolights'],
    focus: 'service-options',
  },
  ...dialogs('full', 'light-power'),
  { flavor: 'slim', name: 'Home, capture off', focus: 'primary' },
  {
    flavor: 'slim',
    name: 'Home, capture on',
    focus: 'primary',
    reach: async (page) => {
      await press(page, 'Enter');
      await expect(page.locator('#primary')).toHaveText('Stop capture');
    },
  },
  {
    flavor: 'slim',
    name: 'Home, no address',
    flags: ['noaddress'],
    focus: 'primary',
  },
  {
    flavor: 'slim',
    name: 'Prysm address',
    focus: 'primary',
    reach: async (page) => {
      await pressUntil(page, 'ArrowRight', 'prysm-address');
      await press(page, 'Enter');
      await expect(page.locator('#dialog-title')).toHaveText('Prysm address');
    },
  },
  ...dialogs('slim', 'primary'),
];

for (const screen of screens) {
  test(`${screen.flavor}: ${screen.name}`, async ({ page }) => {
    await openTv(page, screen.flavor, screen);
    await screen.reach?.(page, screen.flavor);
    await expectSizes(page);
    await expectAllReachable(page);
  });
}
