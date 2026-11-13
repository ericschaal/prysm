// The portal in real browsers against a real prysm-headless per test (see
// prysm.ts). Retries, traces and the HTML report are for CI only.
import { defineConfig, devices } from '@playwright/test';

const ci = !!process.env.CI;

export default defineConfig({
  testDir: '.',
  outputDir: '../.e2e/results',
  timeout: 30_000,
  expect: { timeout: 10_000 },
  forbidOnly: ci,
  retries: ci ? 1 : 0,
  ...(ci ? { workers: 2 } : {}),
  reporter: ci
    ? [['list'], ['html', { outputFolder: '../.e2e/report', open: 'never' }]]
    : 'list',
  use: {
    trace: 'on-first-retry',
    video: 'retain-on-failure',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    { name: 'webkit', use: { ...devices['Desktop Safari'] } },
    // Nightly only (e2e-nightly.yml); `npm run e2e` runs the two above.
    { name: 'firefox', use: { ...devices['Desktop Firefox'] } },
  ],
});
