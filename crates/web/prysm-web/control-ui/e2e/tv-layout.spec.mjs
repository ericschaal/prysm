import { test, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';

for (const [width, height] of [
  [1920, 1080],
  [1280, 720],
]) {
  test(`TV footer stays at the bottom safe margin at ${width}×${height}`, async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'TV viewport check');
    await page.setViewportSize({ width, height });
    await page.addInitScript(() => {
      globalThis.PalmServiceBridge = class {
        call() {
          this.onservicecallback(
            JSON.stringify({
              returnValue: true,
              subscribed: true,
              revision: 1,
              config: {
                receiver: '192.168.18.38:19400',
                source: 'both',
                compression: 'h264-hardware',
                autostart: true,
                metrics: true,
              },
              enabled: true,
              receiverConnected: true,
              capturing: false,
              senderRunning: true,
              stopping: false,
              videoAvailable: false,
              power: 'Active',
              error: null,
              capabilities: { hardwareH264: true },
            }),
          );
        }
        cancel() {
          return undefined;
        }
      };
    });
    await page.route('**/tv-layout/**', async (route) => {
      const path =
        new URL(route.request().url()).pathname.slice('/tv-layout/'.length) ||
        'index.html';
      const file = new URL(
        `../../../prysm-webos/dist/${path}`,
        import.meta.url,
      );
      await route.fulfill({
        body: await readFile(file),
        contentType: path.endsWith('.html')
          ? 'text/html'
          : path.endsWith('.css')
            ? 'text/css'
            : 'text/javascript',
      });
    });
    await page.goto('/tv-layout/');
    await expect(
      page.getByRole('heading', { name: 'Waiting for video' }),
    ).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'Calibrate Colors' }),
    ).toBeVisible();
    const bounds = await page.locator('.key-help').boundingBox();
    expect(Math.abs(bounds.y + bounds.height - height * 0.95)).toBeLessThan(1);
    expect(
      await page.evaluate(
        () => globalThis.document.documentElement.scrollHeight,
      ),
    ).toBeLessThanOrEqual(height);
    await page.screenshot({ path: testInfo.outputPath('tv-layout.png') });
    const start = page.getByRole('button', { name: 'Calibrate Colors' });
    const next = page.getByRole('button', { name: 'Show QR code' });
    const address = page.getByRole('textbox', { name: 'Prysm web address' });
    const back = page.getByRole('button', { name: 'Back', exact: true });
    await expect(
      page
        .getByRole('region', { name: 'Settings' })
        .getByRole('button', { name: 'Calibrate Colors' }),
    ).toBeVisible();
    await page.locator('#primary').focus();
    await page.keyboard.press('ArrowRight');
    await expect(page.locator('#receiver')).toBeFocused();
    for (const id of [
      'source',
      'compression',
      'autostart',
      'calibrate',
      'metrics',
    ]) {
      await page.keyboard.press('ArrowDown');
      await expect(page.locator('#' + id)).toBeFocused();
    }
    await page.keyboard.press('ArrowUp');
    await expect(start).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(next).toBeFocused();
    await expect(address).toHaveValue('http://192.168.18.38:8787');
    await page.keyboard.press('ArrowUp');
    await expect(address).toBeFocused();
    const field = await address.boundingBox();
    expect(field.y + field.height).toBeLessThan((height * 2) / 3);
    await page.keyboard.press('ArrowDown');
    await expect(next).toBeFocused();
    await page.keyboard.press('ArrowLeft');
    await expect(back).toBeFocused();
    await page.keyboard.press('ArrowRight');
    await expect(next).toBeFocused();
    async function safeControls() {
      for (const control of await page
        .locator('.pairing-shell input,.pairing-shell button')
        .all()) {
        const box = await control.boundingBox();
        expect(Math.min(box.width, box.height)).toBeGreaterThanOrEqual(
          width === 1920 ? 75 : 50,
        );
        expect(box.x).toBeGreaterThanOrEqual(20);
        expect(box.y).toBeGreaterThanOrEqual(20);
        expect(box.x + box.width).toBeLessThanOrEqual(width - 20);
        expect(box.y + box.height).toBeLessThanOrEqual(height - 20);
      }
    }
    await safeControls();
    await page.screenshot({ path: testInfo.outputPath('tv-address.png') });
    await page.route('**/calibration-tv.html', (route) =>
      route.fulfill({
        contentType: 'text/html',
        body: '<script>parent.postMessage({type:"calibration",view:{token:"mock",connected:false,step:"choose",shade:0,pairing:"local-preview"}}, "*")</script>',
      }),
    );
    await page.keyboard.press('Enter');
    await expect(
      page.getByRole('img', { name: 'Scan to open the calibration wizard' }),
    ).toBeVisible();
    await expect(back).toBeFocused();
    await safeControls();
    await page.screenshot({ path: testInfo.outputPath('tv-qr.png') });
    await page.keyboard.press('Escape');
    await expect(next).toBeFocused();
    await expect(address).toHaveValue('http://192.168.18.38:8787');
    await page.keyboard.press('Enter');
    await expect(
      page.getByRole('img', { name: 'Scan to open the calibration wizard' }),
    ).toBeVisible();
    await expect(back).toBeFocused();
    const relay = page
      .frames()
      .find((f) => f.url().endsWith('/calibration-tv.html'));
    await relay.evaluate(() =>
      globalThis.parent.postMessage(
        {
          type: 'calibration',
          view: { token: 'mock', connected: true, step: 'choose', shade: 0 },
        },
        '*',
      ),
    );
    await expect(
      page.getByRole('heading', { name: 'Calibration in progress' }),
    ).toBeVisible();
    await expect(page.locator('video')).toHaveCount(0);
    await expect(
      page.getByRole('button', { name: 'Exit calibration' }),
    ).toBeFocused();
    await safeControls();
    await page.screenshot({ path: testInfo.outputPath('tv-standby.png') });
    const select = (shade) =>
      relay.evaluate(
        (shade) =>
          globalThis.parent.postMessage(
            {
              type: 'calibration',
              view: {
                token: 'mock',
                connected: true,
                step: 'colors',
                paused: true,
                shade,
              },
            },
            '*',
          ),
        shade,
      );
    await select(0);
    const video = page.locator('video');
    await expect(video).toHaveAttribute('src', /^blob:/);
    await expect(page.getByText('Changing color...')).toHaveCount(0);
    const frames = await video.evaluateHandle((player) => {
      const ctx = globalThis.document.createElement('canvas').getContext('2d');
      const samples = [];
      function sample() {
        ctx.drawImage(player, 0, 0, 1, 1);
        samples.push([...ctx.getImageData(0, 0, 1, 1).data].slice(0, 3));
        player.requestVideoFrameCallback(sample);
      }
      player.requestVideoFrameCallback(sample);
      return samples;
    });
    await expect
      .poll(() => frames.evaluate((s) => s.some(([r, g]) => r > 180 && g < 40)))
      .toBe(true);
    await select(1);
    await expect(page.getByText('Changing color...')).toBeAttached();
    await expect
      .poll(() => frames.evaluate((s) => s.some(([r, g]) => g > 180 && r < 40)))
      .toBe(true);
    await expect(page.getByText('Changing color...')).toHaveCount(0);
    expect(
      await frames.evaluate((s) => s.every((rgb) => Math.max(...rgb) > 150)),
    ).toBe(true);
    const fallbackRequest = Promise.withResolvers();
    await page.route('**/calibration/green.mp4', fallbackRequest.resolve);
    await video.dispatchEvent('error');
    const fallback = await fallbackRequest.promise;
    const pending = page.getByText('Changing color...');
    await expect(pending).toBeVisible();
    await fallback.fulfill({
      body: await readFile(
        new URL(
          '../../../prysm-webos/dist/calibration/green.mp4',
          import.meta.url,
        ),
      ),
      contentType: 'video/mp4',
    });
    await expect(pending).toHaveCount(0);
    await relay.evaluate(() =>
      globalThis.parent.postMessage(
        {
          type: 'calibration-error',
          terminal: true,
          message: 'Calibration has ended; start again on the TV.',
        },
        '*',
      ),
    );
    await expect(page.getByRole('alert')).toHaveText(
      'Calibration has ended; start again on the TV.',
    );
    await expect(video).toHaveCount(0);
    await page.keyboard.press('Escape');
    await expect(next).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(start).toBeFocused();
    const metrics = page.getByRole('checkbox', { name: 'Prometheus metrics' });
    await metrics.click();
    await expect(
      page.getByRole('button', { name: 'Save changes' }),
    ).toBeVisible();
    const dirtyBounds = await page.locator('.key-help').boundingBox();
    expect(dirtyBounds.y).toBe(bounds.y);
    await metrics.click();
  });

  test(`TV calibration fills ${width}×${height} without inset support`, async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'TV viewport check');
    await page.setViewportSize({ width, height });
    const css = await readFile(
      new URL('../../../prysm-webos/dist/app.css', import.meta.url),
      'utf8',
    );
    // Older TV engines ignore inset; exercise the shipped stylesheet without it.
    await page.setContent(
      `<style>${css.replace(/\binset\s*:[^;}]+;?/g, '')}</style><div id="app"><div class="calibration-screen"><video></video></div></div>`,
    );
    expect(await page.locator('.calibration-screen').boundingBox()).toEqual({
      x: 0,
      y: 0,
      width,
      height,
    });
    expect(await page.locator('video').boundingBox()).toEqual({
      x: 0,
      y: 0,
      width,
      height,
    });
  });
}
