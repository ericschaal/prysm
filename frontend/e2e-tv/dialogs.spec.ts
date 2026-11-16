// Back closes one dialog level and returns focus to the control that opened
// it; Back on Home asks before closing the app (app.tsx, navigation.ts).
import { expect, test, type Page } from '@playwright/test';
import { openTv, press, pressUntil, type Flavor } from './remote';

const start = { full: 'mode-off', slim: 'primary' } as const;
const settingsTop = { full: 'autostart', slim: 'instance' } as const;

const title = (page: Page) => page.locator('#dialog-title');
const modal = (page: Page) => page.locator('#modal');

async function serviceOptions(page: Page, flavor: Flavor) {
  await openTv(page, flavor, { focus: start[flavor] });
  await pressUntil(page, 'ArrowRight', settingsTop[flavor]);
  await pressUntil(page, 'ArrowDown', 'service-options');
  await press(page, 'Enter');
  await expect(title(page)).toHaveText('Service options');
}

for (const flavor of ['full', 'slim'] as const) {
  test.describe(flavor, () => {
    test('Back closes Service options and returns to its row', async ({
      page,
    }) => {
      await serviceOptions(page, flavor);
      await press(page, 'Back');
      await expect(modal(page)).toHaveCount(0);
      await expect(page.locator('#service-options')).toBeFocused();
    });

    for (const [choice, heading] of [
      ['diagnostics', 'Diagnostics'],
      ['cleanup', 'Remove service setup?'],
    ] as const) {
      test(`Back from ${heading} returns to Service options, then to its row`, async ({
        page,
      }) => {
        await serviceOptions(page, flavor);
        await pressUntil(page, 'ArrowDown', choice);
        await press(page, 'Enter');
        await expect(title(page)).toHaveText(heading);
        await press(page, 'Back');
        await expect(title(page)).toHaveText('Service options');
        await expect(page.locator(`#${choice}`)).toBeFocused({ timeout: 2000 });
        await press(page, 'Back');
        await expect(modal(page)).toHaveCount(0);
        await expect(page.locator('#service-options')).toBeFocused();
      });
    }

    test('Back on Home asks before closing; Back again stays', async ({
      page,
    }) => {
      await openTv(page, flavor, { focus: start[flavor] });
      await press(page, 'Back');
      await expect(title(page)).toHaveText('Close Prysm?');
      await expect(page.locator('#stay')).toBeFocused();
      await press(page, 'Back');
      await expect(modal(page)).toHaveCount(0);
      await expect(page.locator(`#${start[flavor]}`)).toBeFocused();
      // From another control: focus goes back there.
      await pressUntil(page, 'ArrowRight', settingsTop[flavor]);
      await press(page, 'Back');
      await press(page, 'Back');
      await expect(page.locator(`#${settingsTop[flavor]}`)).toBeFocused();
      // Close app (OK) closes it; the demo says so in place of the app.
      await press(page, 'Back');
      await press(page, 'ArrowRight');
      await expect(page.locator('#exit')).toBeFocused();
      await press(page, 'Enter', { check: false });
      await expect(page.locator('#root')).toHaveText(
        'The app would close now; lighting is unchanged.',
      );
    });
  });
}

test('slim: Back closes Prysm address and returns to the control that opened it', async ({
  page,
}) => {
  await openTv(page, 'slim', { flags: ['noaddress'], focus: 'primary' });
  await expect(page.locator('#primary')).toHaveText('Add Prysm address');
  await press(page, 'Enter');
  await expect(title(page)).toHaveText('Prysm address');
  await expect(page.locator('#instance-input')).toBeFocused();
  await press(page, 'Back');
  await expect(modal(page)).toHaveCount(0);
  await expect(page.locator('#primary')).toBeFocused();
  await press(page, 'ArrowRight');
  await expect(page.locator('#instance')).toBeFocused();
  await press(page, 'Enter');
  await expect(title(page)).toHaveText('Prysm address');
  await press(page, 'Back');
  await expect(modal(page)).toHaveCount(0);
  await expect(page.locator('#instance')).toBeFocused();
});
