// The guided calibration on a phone-sized page, against a real engine fed by
// a test sender: the user journeys through the wizard, what it saves, and
// that taps answer at once while the TV and the saves follow.
import type { Page, Route } from '@playwright/test';
import { expect, remoteCapture, test, type Prysm } from './prysm';

const ps5 = { id: 'ps5', name: 'PS5', correction: { warmth: 12 } };

test.use({
  viewport: { width: 390, height: 844 },
  settings: {
    capture: remoteCapture,
    output: { kind: 'null' },
    processing: {
      brightness_percent: 50,
      style: { kind: 'natural' },
      sample_density: 128,
      remove_black_bars: true,
      calibrations: [ps5],
      active_calibration: 'ps5',
    },
  },
});

const questions = {
  white: 'Does the wall look as neutral as the screen?',
  red: 'Does the red on the wall match the screen?',
  green: 'Does the green on the wall match the screen?',
  yellow: 'Does the yellow on the wall match the screen?',
  review: 'Name your calibration',
};

async function calibrations(prysm: Prysm) {
  return (await prysm.state()).settings.processing;
}
/** Every number in a correction, flattened: `red.hue` → 4. */
function numbers(value: unknown, at = ''): Record<string, number> {
  if (typeof value === 'number') return { [at]: value };
  return Object.fromEntries(
    Object.entries((value ?? {}) as Record<string, unknown>).flatMap(
      ([key, inner]) =>
        Object.entries(numbers(inner, at ? `${at}.${key}` : key)),
    ),
  );
}

/** Requests to `url` wait until `release` is called (a slow TV or save), then pass. */
async function hold(page: Page, url: string) {
  let release = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const handler = async (route: Route) => {
    await gate;
    await route.continue();
  };
  await page.route(url, handler);
  return release;
}

test.beforeEach(async ({ page, startSender }) => {
  const sender = startSender();
  await page.goto('/');
  await page.getByRole('button', { name: 'Power' }).click();
  await sender.waitFor('connected');
  await expect(
    page.getByRole('button', { name: 'Power', pressed: true }),
  ).toBeVisible();
  await page.goto('/#/calibrate');
  await expect(heading(page)).toHaveText('Match the lights to your TV');
});

const heading = (page: Page) => page.locator('.calibrate h2');
const looksRight = (page: Page) =>
  page.getByRole('button', { name: 'Looks right' });
async function next(page: Page, question: string | RegExp) {
  await looksRight(page).click();
  await expect(heading(page)).toHaveText(question);
}
async function start(page: Page, from = 'New calibration') {
  await page.getByLabel('Start from').selectOption({ label: from });
  await page.getByRole('button', { name: 'Start' }).click();
  await expect(heading(page)).toHaveText(questions.white);
}
/** The wizard's screens (`screens` in calibrate-wizard.ts), review included. */
const screenCount = 11;
/** Looks right until the review, then name it and finish. */
async function finish(page: Page, name: string) {
  for (let i = 0; i < screenCount; i++) {
    if ((await heading(page).textContent()) === questions.review) break;
    await looksRight(page).click();
  }
  await expect(heading(page)).toHaveText(questions.review);
  await page.getByRole('textbox', { name: 'Name' }).fill(name);
  await page.getByRole('button', { name: 'Finish' }).click();
  await expect(heading(page)).toHaveText('Calibration finished');
}

test('the quick pass: Looks right everywhere, then name and finish', async ({
  page,
  prysm,
}) => {
  await start(page);
  await finish(page, 'Movie night');
  await expect(
    page.getByText('“Movie night” is saved and active.'),
  ).toBeVisible();
  const saved = await calibrations(prysm);
  const made = saved.calibrations?.find((each) => each.id !== 'ps5');
  expect(made?.name).toBe('Movie night');
  expect(saved.active_calibration).toBe(made?.id);
  const values = Object.values(numbers(made?.correction));
  expect(values.every((value) => value === 0)).toBe(true);
});

test('adjusting: the value shows, Reset clears it, a held slider lights before release, and it is saved', async ({
  page,
  prysm,
}) => {
  await start(page);
  await next(page, questions.red);
  const hue = page.getByRole('slider', { name: 'More pink or more orange' });
  const value = page.locator('.range', { has: hue }).locator('output');
  await hue.focus();
  for (let tap = 0; tap < 3; tap += 1) await page.keyboard.press('ArrowRight');
  await expect(value).toHaveText('+3');
  const savedHue = async () => {
    const saved = await calibrations(prysm);
    return saved.calibrations?.find(
      (item) => item.id === saved.active_calibration,
    )?.correction.red.hue;
  };
  // Each gesture starts from the value its predecessor persisted.
  await expect.poll(savedHue).toBe(3);
  await page.getByRole('button', { name: 'Reset' }).click();
  await expect(value).toHaveText('0');
  await expect(hue).toHaveValue('0');
  await expect.poll(savedHue).toBe(0);

  // Held: the lights try the value before the key comes up.
  await hue.focus();
  const trial = page.waitForRequest('**/api/v1/processing/trial');
  for (let repeat = 0; repeat < 4; repeat += 1)
    await page.keyboard.down('ArrowRight');
  await trial;
  await page.keyboard.up('ArrowRight');
  await expect(value).toHaveText('+4');
  await finish(page, 'Adjusted');
  const made = (await calibrations(prysm)).calibrations?.find(
    (each) => each.name === 'Adjusted',
  );
  expect(numbers(made?.correction)['red.hue']).toBe(4);
});

test('a check links to its colour, and Looks right there comes back', async ({
  page,
}) => {
  await start(page);
  await next(page, questions.red);
  await next(page, questions.green);
  await next(page, /blue/);
  await next(page, questions.yellow);
  await page.getByRole('button', { name: 'Adjust green' }).click();
  await expect(heading(page)).toHaveText(questions.green);
  await expect(page.getByRole('button', { name: 'Back' })).toHaveCount(0);
  await next(page, questions.yellow);
});

test('Cancel leaves at once when nothing changed, else asks, and restores the calibration before', async ({
  page,
  prysm,
}) => {
  await start(page);
  await page.getByRole('button', { name: 'Cancel calibration' }).click();
  await expect(heading(page)).toHaveText('Calibration cancelled');
  await expect(page.getByRole('button', { name: 'Discard' })).toHaveCount(0);
  expect(await calibrations(prysm)).toMatchObject({
    calibrations: [ps5],
    active_calibration: 'ps5',
  });

  await page.getByRole('button', { name: 'Back to Prysm' }).click();
  await expect(page.getByRole('button', { name: 'Power' })).toBeVisible();
  await page.goto('/#/calibrate');
  await start(page, 'PS5');
  const warmth = page.getByRole('slider', { name: 'Cooler or warmer' });
  await warmth.focus();
  await page.keyboard.press('ArrowLeft');
  await page.getByRole('button', { name: 'Cancel calibration' }).click();
  const question = page.getByRole('group', { name: 'Discard your changes?' });
  await expect(question).toBeVisible();
  await expect(question.getByRole('button', { name: 'Keep' })).toBeFocused();
  await question.getByRole('button', { name: 'Discard' }).click();
  await expect(heading(page)).toHaveText('Calibration cancelled');
  await expect
    .poll(async () => calibrations(prysm))
    .toMatchObject({ calibrations: [ps5], active_calibration: 'ps5' });
});

test('Show without correction toggles and says so', async ({ page, prysm }) => {
  await start(page);
  const compare = page.getByRole('button', { name: 'Show without correction' });
  await compare.click();
  await expect(compare).toHaveAttribute('aria-pressed', 'true');
  await expect
    .poll(async () => (await prysm.state()).calibration?.uncorrected)
    .toBe(true);
  await compare.click();
  await expect(compare).toHaveAttribute('aria-pressed', 'false');
});

test('taps answer at once: the next screen before the TV, a tap during a save', async ({
  page,
}) => {
  await start(page);
  // The TV is slow: the next screen shows while its pattern is on the way.
  let release = await hold(page, '**/api/v1/calibration');
  await looksRight(page).click();
  await expect(heading(page)).toHaveText(questions.red);
  await expect(page.locator('.calibrate-pattern')).toHaveAttribute(
    'aria-busy',
    'true',
  );
  release();
  await expect(page.locator('.calibrate-pattern')).not.toHaveAttribute(
    'aria-busy',
    'true',
  );

  // A slider's save is on its way: Looks right still works.
  release = await hold(page, '**/api/v1/settings');
  const saving = page.waitForRequest('**/api/v1/settings');
  await page.getByRole('slider', { name: 'Softer or stronger' }).focus();
  await page.keyboard.press('ArrowRight');
  await saving;
  await looksRight(page).click();
  await expect(heading(page)).toHaveText(questions.green);
  release();
});

test('the actions stay put and the comparison is in reach on every screen', async ({
  page,
}) => {
  await start(page);
  const compare = page.getByRole('button', { name: 'Show without correction' });
  const tops = new Set<number>();
  for (const question of [questions.red, questions.green, /blue/, /yellow/]) {
    expect(await page.evaluate(() => window.scrollY)).toBe(0);
    const resting = await looksRight(page).boundingBox();
    if (!resting) throw new Error('not laid out');
    tops.add(Math.round(resting.y));
    // A screen taller than the phone scrolls under the bar: at its end the
    // comparison is clear of it.
    await page.evaluate(() =>
      window.scrollTo(0, document.documentElement.scrollHeight),
    );
    const actions = await looksRight(page).boundingBox();
    const tools = await compare.boundingBox();
    if (!actions || !tools) throw new Error('not laid out');
    expect(tools.y + tools.height, 'above the action bar').toBeLessThanOrEqual(
      actions.y,
    );
    await next(page, question);
  }
  expect([...tops]).toHaveLength(1);
});
