// The video source's size and rate as one choice of presets: a preset saves
// its numbers to the settings file; a saved size no preset sets shows as a
// ticked Custom row until a preset replaces it.
import { expect, test } from './prysm';

test('a preset saves its size and rate; the saved custom size shows until replaced', async ({
  page,
  prysm,
  startSender,
}) => {
  // The TV announces what it streams (everything), so the rows unlock.
  startSender();
  await prysm.offered('raw');
  await page.goto('/#/settings/video');
  const editor = page.getByRole('form', { name: 'Video source' });
  const choice = editor.getByRole('radiogroup', { name: 'Size and rate' });
  // A row is clicked as a whole: its radio is drawn as the tick.
  const row = (name: string) => choice.getByText(name, { exact: true });

  // The test's 32 × 18 is no preset: a ticked Custom row says so.
  await expect(choice.getByRole('radio', { name: 'Custom' })).toBeChecked();
  await expect(choice).toContainText(/32\s×\s18 · 30 fps/);
  await expect(choice.getByRole('radio', { name: 'Efficient' })).toBeEnabled();

  await row('Efficient').click();
  await expect(choice.getByRole('radio', { name: 'Efficient' })).toBeChecked();
  await expect(choice.getByRole('radio', { name: 'Custom' })).toHaveCount(0);
  const save = editor.getByRole('button', { name: 'Use this video source' });
  await save.click();
  // Saved: the bar leaves (Cancel stays while saving).
  await expect(editor.getByRole('button', { name: 'Cancel' })).toHaveCount(0);
  const { capture } = await prysm.savedSettings();
  expect(capture).toMatchObject({ width: 320, height: 180, fps: 30 });

  // Arrow keys move within the group, as native radios do.
  await choice.getByRole('radio', { name: 'Efficient' }).focus();
  await page.keyboard.press('ArrowUp');
  await expect(choice.getByRole('radio', { name: 'Smooth' })).toBeChecked();
});
