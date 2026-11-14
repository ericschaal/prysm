// Home's brightness slider, dragged with the mouse and moved with keys, while
// screen sync runs. The drag guards the Safari regression where capturing the
// pointer on the range input stopped the browser's own thumb drag (WebKit bug
// 219636); Chromium never showed it, WebKit does.
import { expect, remoteCapture, test } from './prysm';

test.use({
  settings: {
    capture: remoteCapture,
    output: { kind: 'null' },
    processing: {
      brightness_percent: 50,
      shadow_lift_percent: 0,
      smoothing_seconds: 0.1,
      sample_density: 128,
      spatial_reach_percent: 10,
      remove_black_bars: true,
    },
  },
});

test.beforeEach(async ({ page, startSender }) => {
  const sender = startSender();
  await page.goto('/');
  await page.getByRole('button', { name: 'Power' }).click();
  await sender.waitFor('connected');
  await expect(
    page.getByRole('button', { name: 'Power', pressed: true }),
  ).toBeVisible();
});

test('the brightness thumb drags with the mouse and the result is saved', async ({
  page,
  prysm,
}) => {
  const slider = page.getByRole('slider', { name: 'Brightness' });
  await expect(slider).toHaveValue('50');
  const box = await slider.boundingBox();
  if (!box) throw new Error('the slider is not laid out');
  const y = box.y + box.height / 2;
  // Press on the thumb (the middle, at 50 %), drag left in small steps as a
  // hand would, and let go a quarter of the way along.
  await page.mouse.move(box.x + box.width / 2, y);
  await page.mouse.down();
  for (let step = 1; step <= 10; step += 1)
    await page.mouse.move(box.x + box.width * (0.5 - step * 0.025), y);
  await page.mouse.up();

  const moved = Number(await slider.inputValue());
  expect(moved, 'the thumb followed the drag').toBeLessThan(35);
  // Saved once on release: the engine holds the value the slider shows.
  await expect
    .poll(
      async () => (await prysm.state()).settings.processing.brightness_percent,
    )
    .toBe(moved);
  await expect(slider).toHaveValue(String(moved));
});

test('the brightness slider moves with a held arrow key and the result is saved', async ({
  page,
  prysm,
}) => {
  const slider = page.getByRole('slider', { name: 'Brightness' });
  await slider.focus();
  // Held down: the key repeats, then comes up once.
  for (let repeat = 0; repeat < 5; repeat += 1)
    await page.keyboard.down('ArrowRight');
  await page.keyboard.up('ArrowRight');

  const moved = Number(await slider.inputValue());
  expect(moved).toBeGreaterThan(50);
  await expect
    .poll(
      async () => (await prysm.state()).settings.processing.brightness_percent,
    )
    .toBe(moved);
});

test('quick separate arrow taps all land: the last value is saved', async ({
  page,
  prysm,
}) => {
  const slider = page.getByRole('slider', { name: 'Brightness' });
  await slider.focus();
  // Five taps, each a press and a release, faster than a save goes round.
  for (let tap = 0; tap < 5; tap += 1) {
    await page.keyboard.press('ArrowRight');
    await page.waitForTimeout(250);
  }
  await expect(slider).toHaveValue('55');
  await expect
    .poll(
      async () => (await prysm.state()).settings.processing.brightness_percent,
    )
    .toBe(55);
  // And it stays: nothing later puts an older value back.
  await page.waitForTimeout(1500);
  expect((await prysm.state()).settings.processing.brightness_percent).toBe(55);
  await expect(slider).toHaveValue('55');
});
