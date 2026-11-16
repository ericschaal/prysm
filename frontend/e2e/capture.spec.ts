// Choosing the video source against a real engine: the devices Prysm finds
// (stand-ins with fixed modes, from the `fake_devices` host) and a
// capture-only TV (the test sender), and the sizes and rates each delivers.
import type { Page } from '@playwright/test';
import {
  anyMode,
  chooseKind,
  expect,
  fixedMode,
  test,
  type FakeDevice,
} from './prysm';

const card: FakeDevice = {
  name: 'Capture card',
  path: '/dev/video7',
  modes: [
    fixedMode(1280, 720, 30),
    fixedMode(1280, 720, 60),
    fixedMode(1920, 1080, 30),
    fixedMode(800, 600, 25),
  ],
};
/** A camera without a 16:9 size or 60 fps. */
const webcam: FakeDevice = {
  name: 'Webcam',
  path: '/dev/video8',
  modes: [fixedMode(640, 480, 30)],
};

function editor(page: Page) {
  const form = page.getByRole('form', { name: 'Video source' });
  const sizes = form.getByRole('radiogroup', { name: 'Size and rate' });
  return {
    form,
    devices: form.getByRole('radiogroup', { name: 'Capture device' }),
    sizes,
    size: (name: string) => sizes.getByRole('radio', { name }),
    /** A row is clicked as a whole: its radio is drawn as the tick. */
    row: (name: string) => sizes.getByText(name, { exact: true }),
    save: form.getByRole('button', { name: 'Use this video source' }),
  };
}

test.describe('a capture card', () => {
  // A list is given as `[value, options]`, so Playwright takes it whole.
  test.use({ devices: [[card, webcam], { scope: 'test' }] });

  test('its devices are listed at once; ticking one offers only the presets it delivers, at its sizes', async ({
    page,
    prysm,
  }) => {
    await page.goto('/#/settings/video');
    const video = editor(page);
    await chooseKind(video.form, 'Capture from', 'Video capture device');
    // Found without pressing anything; none ticked yet, so no sizes.
    const card = video.devices.getByRole('radio', { name: 'Capture card' });
    const webcam = video.devices.getByRole('radio', { name: 'Webcam' });
    await expect(card).not.toBeChecked();
    await expect(webcam).not.toBeChecked();
    await expect(video.sizes).toHaveCount(0);

    await video.devices.getByText('Webcam', { exact: true }).click();
    await expect(webcam).toBeChecked();
    // No 60 fps and no wide size: only Balanced, at the camera's size.
    await expect(video.sizes.getByRole('radio')).toHaveCount(1);
    await expect(video.size('Balanced')).toBeChecked();
    await expect(video.sizes).toContainText(/640\s×\s480 · 30 fps/);

    await video.devices.getByText('Capture card', { exact: true }).click();
    await expect(video.size('Smooth')).toBeEnabled();
    await expect(video.sizes.getByRole('radio')).toHaveCount(2);
    await expect(video.sizes).toContainText(/1280\s×\s720 · 60 fps/);
    await video.row('Smooth').click();
    await video.save.click();
    // Saved: the bar leaves (Cancel stays while saving).
    await expect(
      video.form.getByRole('button', { name: 'Cancel' }),
    ).toHaveCount(0);
    expect((await prysm.savedSettings()).capture).toEqual({
      kind: 'device',
      path: '/dev/video7',
      width: 1280,
      height: 720,
      fps: 60,
    });
  });
});

test.describe('a saved size no preset sets', () => {
  test.use({
    devices: [card],
    settings: {
      capture: {
        kind: 'device',
        path: '/dev/video7',
        width: 800,
        height: 600,
        fps: 25,
      },
      output: { kind: 'null' },
    },
  });

  test('shows as one ticked Custom row until a preset replaces it', async ({
    page,
  }) => {
    await page.goto('/#/settings/video');
    const video = editor(page);
    await expect(video.size('Custom')).toBeChecked();
    await expect(video.sizes).toContainText(/800\s×\s600 · 25 fps/);
    await expect(video.save).toHaveCount(0);
    await video.row('Balanced').click();
    await expect(video.size('Balanced')).toBeChecked();
    await expect(video.size('Custom')).toHaveCount(0);
    await expect(video.save).toBeVisible();
  });
});

test.describe('a saved device that is not plugged in', () => {
  test.use({
    devices: [card],
    settings: {
      capture: { kind: 'device', path: '/dev/video9', fps: 60 },
      output: { kind: 'null' },
    },
  });

  test('keeps its row and its sizes, locked, saying why', async ({ page }) => {
    await page.goto('/#/settings/video');
    const video = editor(page);
    const saved = video.devices.getByRole('radio', { name: '/dev/video9' });
    await expect(saved).toBeChecked();
    await expect(saved).toHaveAccessibleDescription('Not plugged in');
    await expect(video.sizes).toContainText(
      'Plug in the device to choose its size and rate.',
    );
    await expect(video.size('Smooth')).toBeChecked();
    await expect(video.size('Smooth')).toBeDisabled();
    await expect(video.size('Balanced')).toBeDisabled();
  });
});

test.describe('a capture-only TV', () => {
  test.use({
    settings: {
      capture: { kind: 'remote', width: 640, height: 360, fps: 30 },
      output: { kind: 'null' },
    },
  });

  test('is not connected until it announces; then its offer decides the presets and encodings', async ({
    page,
    startSender,
  }) => {
    await page.goto('/#/settings/video');
    const video = editor(page);
    await expect(video.sizes).toContainText('The TV isn’t connected.');
    await expect(video.size('Balanced')).toBeChecked();
    await expect(video.size('Balanced')).toBeDisabled();
    await expect(video.size('Smooth')).toBeDisabled();
    await video.form.getByRole('button', { name: 'Advanced' }).click();
    const encoding = video.form.getByLabel('How the TV sends the picture');
    await expect(encoding.locator('option')).toHaveText([
      /video chip/,
      /processor/,
      'Uncompressed',
    ]);

    // The TV's video chip streams up to 30 fps, its processor up to 60, and
    // it sends nothing uncompressed. The page follows without a reload.
    startSender(
      '--offer',
      JSON.stringify({
        h264_hardware: [anyMode(1920, 1080, 30)],
        h264_software: [anyMode(1920, 1080, 60)],
      }),
    );
    await expect(video.size('Balanced')).toBeEnabled();
    await expect(video.sizes).not.toContainText('isn’t connected');
    await expect(video.sizes.getByRole('radio')).toHaveCount(2);
    await expect(video.size('Smooth')).toHaveCount(0);
    await expect(encoding.locator('option')).toHaveText([
      /video chip/,
      /processor/,
    ]);
  });
});
