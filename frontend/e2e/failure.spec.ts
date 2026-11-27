// A capture that fails: the card says why and Power can make a fresh request.
// The sender initially refuses every connection, as a busy TV would.
import { expect, test, type Sender } from './prysm';

const reason = 'The TV is busy';

/** This test's busy TV. */
let busy: Sender;

test.beforeEach(async ({ page, startSender }) => {
  busy = startSender('--refuse', reason);
  await page.goto('/');
  await page.locator('[data-power]').click();
  await busy.waitFor('refused');
});

test('a refused capture is shown, and Power makes a fresh request', async ({
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
    .poll(async () => {
      const { session } = await prysm.state();
      return 'failure' in session ? session.failure?.message : undefined;
    })
    .toContain(reason);
  await expect(card).not.toHaveAttribute('aria-busy', 'true');
  await card.getByText('Details').click();
  await expect(card.getByText(reason)).toBeVisible();

  // The TV is free again: its next connection streams.
  await busy.stop();
  startSender();
  await card.locator('[data-power]').click();
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
      const failure = 'failure' in session ? session.failure : undefined;
      return `${session.state} ${failure?.message ?? 'no failure'}`;
    })
    .toBe('screen_sync no failure');
});

test('a stopped request leaves only Power as a lighting control', async ({
  page,
}) => {
  const card = page.getByRole('region', { name: 'Your lights' });
  await expect(card.getByRole('alert')).toBeVisible();
  await expect(card.locator('[data-power]')).toBeEnabled();
  await expect(card.locator('[data-power]')).toHaveAttribute(
    'aria-pressed',
    'false',
  );
  await expect(card.getByRole('button', { name: 'Try again' })).toHaveCount(0);
  await expect(card.getByRole('radiogroup', { name: 'Mode' })).toHaveCount(0);
  await expect(card.getByRole('slider', { name: 'Brightness' })).toHaveCount(0);
});

test('the video source page shows the failure where it is fixed', async ({
  page,
}) => {
  await page.goto('/#/settings/video');
  const editor = page.getByRole('form', { name: 'Video source' });
  const now = editor.getByRole('status', { name: 'Video source now' });
  await expect(now).toContainText('Video source not working');
  // Prysm's own words, shown as they are.
  await expect(now.getByText(reason)).toBeVisible();
  // Nothing saved here on this visit: no previous source to go back to.
  await expect(editor.getByRole('button', { name: /^Go back to/ })).toHaveCount(
    0,
  );
});
