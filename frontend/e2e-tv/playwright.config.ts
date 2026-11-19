// The TV app (both packages) in desktop Chromium at 1920 × 1080, driven with
// remote keys against the in-memory demo (testkit/tv-demo.tsx): Vite serves
// the demo pages on loopback; no Prysm, TV or WLED is involved.
import { defineConfig, devices } from '@playwright/test';

const ci = !!process.env.CI;
const port = Number(process.env.PRYSM_TV_E2E_PORT ?? 5197);

export default defineConfig({
  testDir: '.',
  outputDir: '../.e2e/tv-results',
  timeout: 60_000,
  expect: { timeout: 10_000 },
  fullyParallel: true,
  forbidOnly: ci,
  retries: ci ? 1 : 0,
  workers: 2,
  reporter: ci
    ? [['list'], ['html', { outputFolder: '../.e2e/tv-report', open: 'never' }]]
    : 'list',
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: 'on-first-retry',
    video: 'retain-on-failure',
  },
  webServer: {
    command: `npx vite hosts --host 127.0.0.1 --port ${port} --strictPort`,
    cwd: '..',
    url: `http://127.0.0.1:${port}/webos/tv-ui/demo.html`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
  projects: [
    {
      name: 'tv-chromium',
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1920, height: 1080 },
        deviceScaleFactor: 1,
      },
    },
  ],
});
