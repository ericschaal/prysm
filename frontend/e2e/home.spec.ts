// Home turns the lights on and off, against a running engine fed by a test
// sender, and switches the style.
import { expect, test } from './prysm';

test('Home turns screen sync on and off', async ({
  page,
  prysm,
  startSender,
}) => {
  const sender = startSender();
  await page.goto('/');
  const card = page.getByRole('region', { name: 'Your lights' });
  const power = card.locator('[data-power]');
  const modes = card.getByRole('radiogroup', { name: 'Mode' });
  // The card's words for screen readers (sighted people see the state word).
  const status = card.locator('[role=status][aria-live]');

  await expect(power).toHaveAttribute('aria-pressed', 'false');
  await expect(status).toHaveText(/^Lights are off/);
  await expect(modes).toHaveCount(0);
  await expect(card.getByRole('slider', { name: 'Brightness' })).toHaveCount(0);

  await power.click();
  await sender.waitFor('connected');
  await expect(power).toHaveAttribute('aria-pressed', 'true');
  await expect(status).toHaveText(/^Lights follow your screen/);
  await expect(modes).toHaveCount(0);
  await expect
    .poll(async () => (await prysm.state()).session.state)
    .toBe('screen_sync');

  await power.click();
  await expect(power).toHaveAttribute('aria-pressed', 'false');
  await expect(status).toHaveText(/^Lights are off/);
  await expect
    .poll(async () => {
      const { target, session } = await prysm.state();
      return `${target} ${session.state}`;
    })
    .toBe('off stopped');
});

test('Home switches the style with one tap or an arrow key, and saves it', async ({
  page,
  prysm,
  startSender,
}) => {
  const sender = startSender();
  await page.goto('/');
  await page.locator('[data-power]').click();
  await sender.waitFor('connected');
  const card = page.getByRole('region', { name: 'Your lights' });
  const style = card.getByRole('radiogroup', { name: 'Style' });
  const choice = (name: string) => style.getByRole('radio', { name });
  const saved = async () =>
    (await prysm.state()).settings.processing.style.kind;

  // Custom is offered only once there is one.
  await expect(style.getByRole('radio')).toHaveText([
    'Natural',
    'Calm',
    'Vivid',
    'Game',
  ]);
  await expect(choice('Natural')).toBeChecked();
  await expect(choice('Game')).toHaveAccessibleDescription(
    'Reacts at once, with the least delay.',
  );

  await choice('Game').click();
  await expect(choice('Game')).toBeChecked();
  await expect.poll(saved).toBe('game');

  await choice('Game').focus();
  await page.keyboard.press('ArrowLeft');
  await expect(choice('Vivid')).toBeChecked();
  await expect(choice('Vivid')).toBeFocused();
  await expect.poll(saved).toBe('vivid');

  await card.getByRole('button', { name: 'Adjust style' }).click();
  await expect(page).toHaveURL(/#\/settings\/style$/);
});
