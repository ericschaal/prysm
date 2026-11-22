// A capture that fails: the card says so, offers Try again, and stays usable.
// The sender refuses every connection, as a TV whose capture is busy does,
// so lighting started again (a settings save retries it) fails again too.
import { expect, test, type Sender } from './prysm';

const reason = 'The TV is busy';

/** This test's busy TV. */
let busy: Sender;

test.beforeEach(async ({ page, startSender }) => {
  busy = startSender('--refuse', reason);
  await page.goto('/');
  await page.getByRole('button', { name: 'Power' }).click();
  await busy.waitFor('refused');
});

test('a refused capture is shown, and Try again starts screen sync', async ({
  page,
  prysm,
  startSender,
}) => {
  const card = page.getByRole('region', { name: 'Your lights' });
  await expect(card.getByRole('alert')).toBeVisible();
  // Settled first: the engine holds the failure and the Power click has its
  // answer. The line can show the state's failure before the click's own
  // reply lands, and opening Details in between raced with that update.
  await expect
    .poll(async () => (await prysm.state()).session.failure?.message)
    .toContain(reason);
  await expect(card).not.toHaveAttribute('aria-busy', 'true');
  await card.getByText('Details').click();
  await expect(card.getByText(reason)).toBeVisible();

  // The TV is free again: its next connection streams.
  await busy.stop();
  startSender();
  await card.getByRole('button', { name: 'Try again' }).click();
  // The card's words for screen readers (the problem line folds away).
  await expect(card.locator('[role=status][aria-live]')).toHaveText(
    /^Lights follow your screen/,
  );
  await expect(
    card.getByRole('button', { name: 'Power', pressed: true }),
  ).toBeVisible();
  await expect
    .poll(async () => {
      const { session } = await prysm.state();
      return `${session.state} ${session.failure?.message ?? 'no failure'}`;
    })
    .toBe('screen_sync no failure');
});

test('changing brightness on a failed card does not leave it loading', async ({
  page,
  prysm,
}) => {
  const card = page.getByRole('region', { name: 'Your lights' });
  await expect(card.getByRole('alert')).toBeVisible();
  const slider = page.getByRole('slider', { name: 'Brightness' });
  await slider.focus();
  await slider.press('ArrowLeft');
  const changed = Number(await slider.inputValue());
  await expect
    .poll(
      async () => (await prysm.state()).settings.processing.brightness_percent,
    )
    .toBe(changed);

  // Nothing is in progress: the card, its power button and Try again are idle.
  await expect(card).toHaveAttribute('aria-busy', 'false');
  await expect(card.getByRole('button', { name: 'Power' })).toHaveAttribute(
    'aria-busy',
    'false',
  );
  await expect(card.getByRole('alert')).toBeVisible();
  await expect(card.getByRole('button', { name: 'Try again' })).toBeEnabled();
});

test('the video source page shows the failure where it is fixed', async ({
  page,
}) => {
  await page.goto('/#/settings/video');
  const editor = page.getByRole('form', { name: 'Video source' });
  const now = editor.getByRole('status', { name: 'Video source now' });
  await expect(now).toContainText('No picture');
  // Prysm's own words, shown as they are.
  await expect(now.getByText(reason)).toBeVisible();
  // Nothing saved here on this visit: no previous source to go back to.
  await expect(editor.getByRole('button', { name: /^Go back to/ })).toHaveCount(
    0,
  );
});
