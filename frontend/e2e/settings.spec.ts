// Settings against a real engine fed by a test sender: one column of rows in
// three groups under one save status; the video source changed on its own
// page, saved with one button, its status following screen sync live.
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
  // Brightness and the calibration choice are on Home only.
  await expect(page.getByRole('slider', { name: 'Brightness' })).toHaveCount(0);

  const bars = page
    .getByRole('region', { name: 'Picture' })
    .getByRole('checkbox', { name: 'Remove black bars' });
  const before = await bars.isChecked();
  await bars.click();
  await expect(page.getByText(/^(Saving…|Saved)$/)).toBeVisible();
  await expect(status).toBeVisible();
  const state = await prysm.state();
  expect(state.settings.processing.remove_black_bars).toBe(!before);

  // A row is the link: its label, then its value.
  await page.getByRole('button', { name: /^Lights/ }).click();
  await expect(page).toHaveURL(/#\/settings\/lights$/);
  await expect(page.getByRole('form', { name: 'Your lights' })).toBeVisible();
});

test('the video source page shows whether it works, before and after a save', async ({
  page,
  prysm,
  startSender,
}) => {
  const sender = startSender();
  await page.goto('/#/settings');
  await page.getByRole('button', { name: /^Video source/ }).click();
  await expect(page).toHaveURL(/#\/settings\/video$/);

  const editor = page.getByRole('form', { name: 'Video source' });
  const now = editor.getByRole('status', { name: 'Video source now' });
  await expect(now).toContainText(
    'Screen sync is off, so this source isn’t in use.',
  );
  // The sender has announced what the TV streams: the presets are open.
  const efficient = editor.getByRole('radio', { name: 'Efficient' });
  await expect(efficient).toBeEnabled();
  await editor.getByText('Efficient', { exact: true }).click();
  const save = editor.getByRole('button', { name: 'Use this video source' });
  await save.click();
  // Saved: the bar says so where the user is, then goes; nothing to confirm.
  await expect(editor.getByRole('button', { name: 'Cancel' })).toHaveCount(0);
  await expect(editor.getByText(/^Saved\. Screen sync is off/)).toBeVisible();
  await expect(editor.getByText(/^Saved\./)).toHaveCount(0);
  const saved = await prysm.state();
  expect(saved.settings.capture).toMatchObject({
    width: 320,
    height: 180,
    fps: 30,
  });

  // Screen sync on (from Home, or here another device): the status follows.
  const response = await page.request.put('/api/v1/target', {
    data: { target: 'screen_sync' },
  });
  expect(response.status()).toBe(204);
  await sender.waitFor('connected');
  await expect(now).toContainText('Picture arriving');
  await expect(now).toContainText('320 × 180');
  // The card itself opens the large picture.
  await now.getByRole('button', { name: /See the picture$/ }).click();
  await expect(page).toHaveURL(/#\/preview$/);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page).toHaveURL(/#\/settings$/);
});
