// Settings: the video source is changed on its own page, saved with one
// button, then tried on request and said to work, against a real engine fed
// by a test sender.
import { expect, test } from './prysm';

test('a saved video source is tried on request and said to work', async ({
  page,
  prysm,
  startSender,
}) => {
  const sender = startSender();
  await page.goto('/#/settings');
  await expect(page.getByText('All changes saved')).toBeVisible();
  await page.getByRole('button', { name: 'Video source', exact: true }).click();
  await expect(page).toHaveURL(/#\/settings\/video$/);

  const editor = page.getByRole('form', { name: 'Video source' });
  const status = editor.getByRole('status');
  await editor.getByLabel('Frame rate').fill('25');
  await editor.getByRole('button', { name: 'Use this video source' }).click();
  await expect(status).toHaveText(
    'Saved. Screen sync is off, so this source hasn’t been tried.',
  );
  const saved = (await prysm.state()) as unknown as {
    settings: { capture: { fps: number } };
  };
  expect(saved.settings.capture.fps).toBe(25);

  await editor
    .getByRole('button', { name: 'Turn on screen sync to try' })
    .click();
  await sender.waitFor('connected');
  await expect(status).toHaveText('Saved. The picture is arriving.');
  await editor.getByRole('button', { name: 'Done' }).click();
  await expect(page).toHaveURL(/#\/settings$/);
});
