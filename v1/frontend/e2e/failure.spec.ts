// A capture that fails: the card says so, offers Try again, and stays usable.
// The sender refuses its first connection, as a TV whose capture is busy does.
import { expect, test } from './prysm';

const reason = 'The TV is busy';

test.beforeEach(async ({ page, startSender }) => {
  const sender = startSender('--refuse', '1', reason);
  await page.goto('/');
  await page.getByRole('button', { name: 'Power' }).click();
  await sender.waitFor('refused');
});

test('a refused capture is shown, and Try again starts screen sync', async ({
  page,
  prysm,
}) => {
  const card = page.getByRole('region', { name: 'Your lights' });
  await expect(card.getByRole('alert')).toBeVisible();
  await card.getByText('Details').click();
  await expect(card.getByText(reason)).toBeVisible();
  await expect
    .poll(async () => (await prysm.state()).failure?.message)
    .toContain(reason);

  // The sender's next connection streams.
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
      const { session, failure } = await prysm.state();
      return `${session.state} ${failure?.message ?? 'no failure'}`;
    })
    .toBe('running no failure');
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
