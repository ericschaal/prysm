// Home turns the lights on and off, against a running engine fed by a test sender.
import { expect, test } from './prysm';

test('Home turns screen sync on and off', async ({
  page,
  prysm,
  startSender,
}) => {
  const sender = startSender();
  await page.goto('/');
  const card = page.getByRole('region', { name: 'Your lights' });
  const power = card.getByRole('button', { name: 'Power' });
  // The card's words for screen readers (sighted people see the state word).
  const status = card.locator('[role=status][aria-live]');

  await expect(power).toHaveAttribute('aria-pressed', 'false');
  await expect(status).toHaveText(/^Lights are off/);

  await power.click();
  await sender.waitFor('connected');
  await expect(power).toHaveAttribute('aria-pressed', 'true');
  await expect(status).toHaveText(/^Lights follow your screen/);
  await expect
    .poll(async () => (await prysm.state()).session.state)
    .toBe('running');

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
