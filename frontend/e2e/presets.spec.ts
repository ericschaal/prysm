// The video source's size and rate as one choice of presets: a preset saves
// its numbers to the settings file, and Custom opens the number fields below
// the list without moving it.
import { expect, test } from './prysm';

test('a preset saves its size and rate; Custom opens the fields below the list', async ({
  page,
  prysm,
}) => {
  await page.goto('/#/settings/video');
  const editor = page.getByRole('form', { name: 'Video source' });
  const choice = editor.getByRole('radiogroup', { name: 'Size and rate' });
  const width = editor.getByRole('spinbutton', { name: 'Width' });
  // A row is clicked as a whole: its radio is drawn as the tick.
  const row = (name: string) => choice.getByText(name, { exact: true });

  // The test's 32 × 18 is no preset: Custom, with the fields open.
  await expect(choice.getByRole('radio', { name: 'Custom' })).toBeChecked();
  await expect(width).toHaveValue('32');

  await row('Efficient').click();
  await expect(choice.getByRole('radio', { name: 'Efficient' })).toBeChecked();
  await expect(width).toHaveCount(0);
  const save = editor.getByRole('button', { name: 'Use this video source' });
  await save.click();
  // Saved: the bar leaves (Cancel stays while saving).
  await expect(editor.getByRole('button', { name: 'Cancel' })).toHaveCount(0);
  const saved = (await prysm.savedSettings()) as {
    capture: { width: number; height: number; fps: number };
  };
  expect([
    saved.capture.width,
    saved.capture.height,
    saved.capture.fps,
  ]).toEqual([320, 180, 30]);

  // Where the list is on the page (not the screen: the save bar going may
  // scroll it), and its size.
  const place = () =>
    choice.evaluate((list) => {
      const box = list.getBoundingClientRect();
      return [box.top + window.scrollY, box.height];
    });
  const before = await place();
  await row('Custom').click();
  await expect(width).toHaveValue('320');
  expect(await place()).toEqual(before);

  // Arrow keys move within the group, as native radios do.
  await choice.getByRole('radio', { name: 'Custom' }).focus();
  await page.keyboard.press('ArrowUp');
  await expect(choice.getByRole('radio', { name: 'Efficient' })).toBeChecked();
  await expect(width).toHaveCount(0);
});
