// Settings against a real engine fed by a test sender: one column of rows in
// three groups under one save status; the video source changed on its own
// page, saved with one button, then tried on request and said to work.
import { expect, test } from './prysm';

test('Settings lists its groups in order, and says whether it is saved', async ({
  page,
  prysm,
}) => {
  await page.goto('/#/settings');
  await expect(page.getByRole('heading', { level: 2 })).toHaveText([
    'Settings',
    'Setup',
    'Picture',
    'This device',
  ]);
  const status = page.getByText('All changes saved');
  await expect(status).toBeVisible();
  // Brightness and the profile choice are on Home only.
  await expect(page.getByRole('slider', { name: 'Brightness' })).toHaveCount(0);

  const bars = page
    .getByRole('region', { name: 'Picture' })
    .getByRole('checkbox', { name: 'Remove black bars' });
  const before = await bars.isChecked();
  await bars.click();
  await expect(page.getByText(/^(Saving…|Saved)$/)).toBeVisible();
  await expect(status).toBeVisible();
  const state = (await prysm.state()) as unknown as {
    settings: { processing: { remove_black_bars: boolean } };
  };
  expect(state.settings.processing.remove_black_bars).toBe(!before);

  // A row is the link: its label, then its value.
  await page.getByRole('button', { name: /^Lights/ }).click();
  await expect(page).toHaveURL(/#\/settings\/lights$/);
  await expect(page.getByRole('form', { name: 'Your lights' })).toBeVisible();
});

test('a saved video source is tried on request and said to work', async ({
  page,
  prysm,
  startSender,
}) => {
  const sender = startSender();
  await page.goto('/#/settings');
  await page.getByRole('button', { name: /^Video source/ }).click();
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
