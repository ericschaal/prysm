import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import { ApplicationError } from '@prysm/application-client';
import { processingDelayMs } from '@prysm/web-portal';
import {
  initialWizard,
  losesWork,
  placeAfter,
  screenOf,
  screens,
  wizardReducer,
} from '../packages/web-portal/calibrate-wizard';
import {
  $,
  $$,
  button,
  edit,
  errorOf,
  field,
  openPage,
  renderPortal,
  row,
  sleep,
  text,
} from './dom';

const capture = { kind: 'device', path: '/dev/video0' };
const output = {
  kind: 'wled',
  address: '192.0.2.1',
  leds: { top: 10, left: 4 },
};
const unchanged = { hue: 0, strength: 0 };
const zero = {
  warmth: 0,
  tint: 0,
  midtones: 0,
  red: unchanged,
  green: unchanged,
  blue: unchanged,
};
const ps5 = { id: 'ps5', name: 'PS5', correction: { ...zero, warmth: 12 } };
const settle = () => act(async () => sleep(processingDelayMs + 100));
/** Click and let the command finish and the stream deliver. */
const press = (element) =>
  act(async () => {
    element.click();
    await sleep(5);
  });
const processing = (fixture) => fixture.snapshot().settings.processing;
const patterns = (fixture) =>
  fixture.calls.filter((call) => call.startsWith('calibration:'));
const heading = () => $('.calibrate h2').textContent;
const step = () => $('.setup-steps [aria-current="step"]').textContent;
const sliders = () =>
  $$('.calibrate input[type="range"]').map((input) => input.name);
const draftField = (name) =>
  field(`processing.calibrations[0].correction.${name}`);
const questions = {
  white: 'Does the wall look as neutral as the screen?',
  red: 'Does the red on the wall match the screen?',
  green: 'Does the green on the wall match the screen?',
  blue: 'Does the blue on the wall match the screen?',
  yellow: 'Does the yellow on the wall match the screen?',
  mixed: 'Do both colours on the wall match the screen?',
  review: 'Name your calibration',
};

/** Screen sync running, optionally with saved calibrations; opens the Calibrate page. */
async function openCalibrate({
  target = 'processed',
  calibrations,
  Host = HeadlessWebPortal,
} = {}) {
  const portal = await renderPortal(Host, { capture, output }, (snapshot) => ({
    target,
    session: { state: target === 'processed' ? 'running' : 'stopped' },
    settings: {
      ...snapshot.settings,
      processing: {
        ...snapshot.settings.processing,
        ...(calibrations
          ? { calibrations, active_calibration: calibrations[0].id }
          : {}),
      },
    },
  }));
  await openPage('Calibrations');
  await openPage('New calibration');
  return portal;
}

test('Calibrate opens from the portal at #/calibrate on both hosts', async () => {
  for (const Host of [HeadlessWebPortal, WebosWebPortal]) {
    const portal = await openCalibrate({ Host });
    try {
      assert.equal(window.location.hash, '#/calibrate');
      assert.equal(heading(), 'Match the lights to your TV');
      assert.equal(document.activeElement, $('.calibrate h2'));
      assert(text().includes('calibrate in the mode you watch in'));
      assert(!field('calibration.from'), 'no question without calibrations');
      await press(button('Leave'));
      assert(!$('.calibrate'));
      assert(row('New calibration'), 'back in Calibrations');
      assert.deepEqual(patterns(portal.fixture), []);
    } finally {
      await portal.unmount();
    }
  }
});

test("the TV's link opens Calibrate directly; leaving returns Home", async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  try {
    await act(async () => {
      window.location.hash = '#/calibrate';
      await sleep(0);
    });
    assert.equal(heading(), 'Match the lights to your TV');
    await press(button('Leave'));
    assert(!$('.calibrate'));
    assert(['', '#/'].includes(window.location.hash), 'Home');
    assert($('.light-card'), 'Home');
  } finally {
    await portal.unmount();
  }
});

test('a new calibration is made step by step and finished under its name', async () => {
  const portal = await openCalibrate();
  const { fixture } = portal;
  try {
    await press(button('Start'));
    const [draft] = processing(fixture).calibrations;
    assert.deepEqual(draft, {
      id: draft.id,
      name: 'Calibrated',
      correction: zero,
    });
    assert.equal(processing(fixture).active_calibration, draft.id);
    assert.deepEqual(patterns(fixture), ['calibration:gray_192']);
    assert.deepEqual(fixture.snapshot().calibration, {
      pattern: 'gray_192',
      uncorrected: false,
    });
    assert.equal(heading(), questions.white);
    assert.equal($('.calibrate-pattern').textContent, 'On your TV: light grey');
    assert.equal(step(), 'White');
    // The top line names the flow; the progress bar alone says where.
    assert.equal($('.calibrate .eyebrow').textContent, 'Calibrate colours');
    assert.equal(
      $('.setup-steps').getAttribute('aria-label'),
      'Calibration progress',
    );
    assert.deepEqual(sliders(), [
      'processing.calibrations[0].correction.warmth',
      'processing.calibrations[0].correction.tint',
    ]);
    // White's sliders are named by their ends too, with the value between.
    const ends = (input) =>
      $$('[aria-hidden="true"]', input.closest('.range')).map(
        (end) => end.textContent,
      );
    const helpOf = (input) =>
      document.getElementById(input.getAttribute('aria-describedby'))
        .textContent;
    const warmth = draftField('warmth');
    assert.equal(warmth.labels[0].textContent, 'Cooler or warmer');
    assert.deepEqual(ends(warmth), ['Cooler', 'Warmer']);
    assert.equal(
      helpOf(warmth),
      'Too blue? Toward warmer. Too yellow? Toward cooler.',
    );
    assert.equal($('output', warmth.closest('.range')).textContent, '0');
    const tint = draftField('tint');
    assert.equal(tint.labels[0].textContent, 'More green or more pink');
    assert.deepEqual(ends(tint), ['More green', 'More pink']);
    assert.equal(
      helpOf(tint),
      'Too pink? Toward green. Too green? Toward pink.',
    );

    // Slider edits are live saves on the draft calibration.
    await edit(draftField('warmth'), 20);
    await edit(draftField('tint'), -5);
    await settle();
    assert.deepEqual(processing(fixture).calibrations[0].correction, {
      ...zero,
      warmth: 20,
      tint: -5,
    });

    // Compare without correction, and back.
    const compare = button('Show without correction');
    await press(compare);
    assert.equal(compare.getAttribute('aria-pressed'), 'true');
    await press(compare);
    assert.equal(compare.getAttribute('aria-pressed'), 'false');
    assert.deepEqual(patterns(fixture).slice(1), [
      'calibration:gray_192:uncorrected',
      'calibration:gray_192',
    ]);

    // Each primary: two sliders named by their ends, on its pair.
    fixture.calls.length = 0;
    await press(button('Looks right'));
    assert.equal(heading(), questions.red);
    assert.equal(
      $('.calibrate-pattern').textContent,
      'On your TV: red, moderate left, vivid right',
    );
    assert.deepEqual(sliders(), [
      'processing.calibrations[0].correction.red.hue',
      'processing.calibrations[0].correction.red.strength',
    ]);
    const hue = draftField('red.hue');
    assert.equal(hue.labels[0].textContent, 'More pink or more orange');
    // What to do with each slider is the line under it.
    assert.equal(
      document.getElementById(hue.getAttribute('aria-describedby')).textContent,
      'Too orange? Toward pink. Too pink? Toward orange.',
    );
    assert.deepEqual(
      $$('[aria-hidden="true"]', hue.closest('.range')).map(
        (end) => end.textContent,
      ),
      ['More pink', 'More orange'],
    );
    assert.equal(
      draftField('red.strength').labels[0].textContent,
      'Softer or stronger',
    );
    await edit(hue, 8);
    await edit(draftField('red.strength'), -12);
    await settle();
    assert.deepEqual(processing(fixture).calibrations[0].correction.red, {
      hue: 8,
      strength: -12,
    });

    await press(button('Looks right'));
    assert.equal(heading(), questions.green);
    assert.equal(
      draftField('green.hue').labels[0].textContent,
      'More yellow or more turquoise',
    );
    await press(button('Looks right'));
    assert.equal(heading(), questions.blue);
    assert.equal(
      draftField('blue.hue').labels[0].textContent,
      'More turquoise or more purple',
    );

    // A check has no sliders: it asks, and links to what it is made of.
    await press(button('Looks right'));
    assert.equal(heading(), questions.yellow);
    assert.equal(step(), 'Checks · 1 of 6');
    assert.equal($('.calibrate-pattern').textContent, 'On your TV: yellow');
    assert.deepEqual(sliders(), []);
    assert(!button('Reset'), 'nothing to reset on a check');
    assert.deepEqual(
      $$('.calibrate-fixes button').map((link) => link.textContent),
      ['Adjust red', 'Adjust green'],
    );

    // Adjusting from a check comes back to it.
    await press(button('Adjust green'));
    assert.equal(heading(), questions.green);
    assert(!button('Back'), 'the way on is back to the check');
    await edit(draftField('green.strength'), 30);
    await settle();
    await press(button('Looks right'));
    assert.equal(heading(), questions.yellow);
    assert(button('Back'));
    await press(button('Looks right'));
    assert.equal(heading(), 'Does the cyan on the wall match the screen?');
    await press(button('Looks right'));
    assert.equal(heading(), 'Does the magenta on the wall match the screen?');
    await press(button('Looks right'));
    assert.equal(heading(), questions.mixed);
    assert.deepEqual(
      $$('.calibrate-fixes button').map((link) => link.textContent),
      ['Adjust white', 'Adjust red', 'Adjust green', 'Adjust blue'],
    );
    await press(button('Adjust white'));
    assert.equal(heading(), questions.white);
    await press(button('Looks right'));
    assert.equal(heading(), questions.mixed);
    await press(button('Looks right'));
    await press(button('Looks right'));
    await press(button('Looks right'));
    assert.equal(heading(), questions.review);
    assert(!button('Looks right'));
    await press(button('Back'));
    assert.equal(heading(), questions.mixed);
    assert.equal(step(), 'Checks · 6 of 6');
    assert.equal(
      $('.calibrate-pattern').textContent,
      'On your TV: amber left, violet right',
    );
    await press(button('Looks right'));
    assert.deepEqual(patterns(fixture), [
      'calibration:red_pair',
      'calibration:green_pair',
      'calibration:blue_pair',
      'calibration:yellow',
      'calibration:green_pair',
      'calibration:yellow',
      'calibration:cyan',
      'calibration:magenta',
      'calibration:coast',
      'calibration:gray_192',
      'calibration:coast',
      'calibration:garden',
      'calibration:dusk',
      'calibration:review_grid',
      'calibration:dusk',
      'calibration:review_grid',
    ]);

    const name = field('calibration.name');
    assert.equal(name.value, 'Calibrated');
    // A name problem is said at the field, and Finish goes there.
    await edit(name, '   ');
    assert.equal(errorOf(name), 'Enter a name.');
    await press(button('Finish'));
    assert.equal(document.activeElement, name);
    assert.equal(heading(), questions.review, 'not finished');
    await edit(name, '  Movie night ');
    assert.equal(errorOf(name), undefined, 'the name is fine again');
    await press(button('Finish'));
    await act(async () => sleep(5));
    assert.deepEqual(processing(fixture).calibrations, [
      {
        id: draft.id,
        name: 'Movie night',
        correction: {
          ...zero,
          warmth: 20,
          tint: -5,
          red: { hue: 8, strength: -12 },
          green: { hue: 0, strength: 30 },
        },
      },
    ]);
    assert.equal(processing(fixture).active_calibration, draft.id);
    assert.equal(patterns(fixture).at(-1), 'calibration:end');
    assert.equal('calibration' in fixture.snapshot(), false);
    assert.equal(heading(), 'Calibration finished');
    assert(text().includes('“Movie night” is saved and active.'));
    await press(button('Back to Prysm'));
    assert(!$('.calibrate'));
  } finally {
    await portal.unmount();
  }
});

test("Reset puts back only the screen's own controls", async () => {
  const portal = await openCalibrate();
  const { fixture } = portal;
  try {
    await press(button('Start'));
    assert.equal(button('Reset').disabled, true, 'nothing to reset yet');
    await edit(draftField('warmth'), 15);
    await edit(draftField('tint'), 10);
    assert.equal(button('Reset').disabled, false);
    await press(button('Looks right'));
    await edit(draftField('red.hue'), -20);
    await edit(draftField('red.strength'), 40);
    await press(button('Looks right'));
    assert.equal(button('Reset').disabled, true, 'green is unchanged');
    await edit(draftField('green.hue'), 7);
    await press(button('Reset'));
    assert.equal(draftField('green.hue').value, '0');
    await press(button('Back'));
    await press(button('Reset'));
    assert.equal(button('Reset').disabled, true);
    await settle();
    assert.deepEqual(processing(fixture).calibrations[0].correction, {
      ...zero,
      warmth: 15,
      tint: 10,
    });
    await press(button('Back'));
    await press(button('Reset'));
    await settle();
    assert.deepEqual(processing(fixture).calibrations[0].correction, zero);
  } finally {
    await portal.unmount();
  }
});

test('a held slider lights its values at once and saves on release', async () => {
  // Without this the lights followed only 0.4 s after the slider stopped.
  const portal = await openCalibrate();
  const { fixture, dom } = portal;
  const fire = (target, type) =>
    act(async () =>
      target.dispatchEvent(new dom.window.Event(type, { bubbles: true })),
    );
  const trials = () =>
    fixture.calls.filter((call) => call.startsWith('trial:')).length;
  try {
    await press(button('Start'));
    await press(button('Looks right'));
    const slider = () => draftField('red.strength');
    const before = trials();
    await fire(slider(), 'pointerdown');
    await edit(slider(), -30);
    assert.equal(trials(), before + 1, 'tried on the lights while held');
    assert.equal(
      processing(fixture).calibrations[0].correction.red.strength,
      0,
      'not saved while held',
    );
    await fire(dom.window, 'pointerup');
    await settle();
    assert.equal(
      processing(fixture).calibrations[0].correction.red.strength,
      -30,
      'saved on release',
    );
  } finally {
    await portal.unmount();
  }
});

test('starting from a saved calibration works on a copy; Cancel deletes it and restores the previous one', async () => {
  const portal = await openCalibrate({ calibrations: [ps5] });
  const { fixture } = portal;
  try {
    await edit(field('calibration.from'), 'ps5');
    await press(button('Start'));
    const [, copy] = processing(fixture).calibrations;
    assert.notEqual(copy.id, 'ps5');
    assert.deepEqual(copy, {
      id: copy.id,
      name: 'PS5 calibrated',
      correction: ps5.correction,
    });
    assert.equal(processing(fixture).active_calibration, copy.id);
    assert.deepEqual(patterns(fixture), ['calibration:gray_192']);

    await press(button('Looks right'));
    // Nothing adjusted: nothing to lose, so no question.
    await press(button('Cancel calibration'));
    await act(async () => sleep(5));
    assert(!button('Discard'), 'not asked');
    assert.deepEqual(processing(fixture).calibrations, [ps5]);
    assert.equal(processing(fixture).active_calibration, 'ps5');
    assert.equal(patterns(fixture).at(-1), 'calibration:end');
    assert.equal('calibration' in fixture.snapshot(), false);
    assert.equal(heading(), 'Calibration cancelled');
  } finally {
    await portal.unmount();
  }
});

test('Cancel asks before discarding changes, in its own line', async () => {
  const portal = await openCalibrate();
  const { fixture, dom } = portal;
  const keyDown = (target, key) =>
    act(async () =>
      target.dispatchEvent(
        new dom.window.KeyboardEvent('keydown', { key, bubbles: true }),
      ),
    );
  try {
    await press(button('Start'));
    await edit(draftField('warmth'), 10);
    await settle();
    await press(button('Cancel calibration'));
    const question = $('.calibrate-confirm');
    assert.equal(
      document.getElementById(question.getAttribute('aria-labelledby'))
        .textContent,
      'Discard your changes?',
    );
    assert.equal(question.getAttribute('role'), 'group');
    assert.equal(document.activeElement, button('Keep'), 'the safe answer');
    assert.equal(heading(), questions.white, 'nothing else changed');
    assert.deepEqual(patterns(fixture), ['calibration:gray_192']);

    // Keep: back as it was, focus on Cancel; Escape keeps too.
    await press(button('Keep'));
    assert.equal(document.activeElement, button('Cancel calibration'));
    assert.equal(processing(fixture).calibrations.length, 1);
    await press(button('Cancel calibration'));
    await keyDown(button('Keep'), 'Escape');
    assert(button('Cancel calibration'));

    await press(button('Cancel calibration'));
    await press(button('Discard'));
    await act(async () => sleep(5));
    assert.equal(heading(), 'Calibration cancelled');
    assert.equal(processing(fixture).calibrations, undefined);
    assert.equal(patterns(fixture).at(-1), 'calibration:end');
  } finally {
    await portal.unmount();
  }
});

test('a tap moves on at once; the TV pattern follows, and a refusal goes back', async () => {
  const portal = await openCalibrate();
  const { fixture } = portal;
  const tap = (element) => act(async () => element.click());
  const line = () => $('.calibrate-pattern');
  try {
    await press(button('Start'));
    fixture.calls.length = 0;
    let release = fixture.holdPatterns();
    await tap(button('Looks right'));
    assert.equal(heading(), questions.red, 'before the TV answers');
    assert.equal(line().getAttribute('aria-busy'), 'true', 'on its way');
    assert.equal(button('Looks right').disabled, false);
    // A second tap meanwhile: the TV gets it after the first, nothing between.
    await tap(button('Looks right'));
    assert.equal(heading(), questions.green);
    release();
    await act(async () => sleep(5));
    assert.equal(line().hasAttribute('aria-busy'), false, 'the TV shows it');
    assert.deepEqual(patterns(fixture), [
      'calibration:red_pair',
      'calibration:green_pair',
    ]);
    assert.equal(fixture.snapshot().calibration.pattern, 'green_pair');

    // The comparison is shown as on at once.
    release = fixture.holdPatterns();
    const compare = button('Show without correction');
    await tap(compare);
    assert.equal(compare.getAttribute('aria-pressed'), 'true');
    assert.equal(compare.disabled, false);
    release();
    await act(async () => sleep(5));
    assert.equal(fixture.snapshot().calibration.uncorrected, true);

    // Refused: back to what the TV shows, saying why.
    fixture.failNext({ kind: 'unavailable', message: 'no answer' });
    await press(button('Looks right'));
    assert.equal(heading(), questions.green);
    assert.equal(compare.getAttribute('aria-pressed'), 'true');
    assert.equal(
      $('.calibrate [role="alert"]').textContent,
      'The TV pattern did not change: no answer',
    );
    await press(button('Looks right'));
    assert.equal(heading(), questions.blue);
  } finally {
    await portal.unmount();
  }
});

test('a tap while a slider saves is not dropped', async () => {
  // All buttons were disabled during these saves, so a tap then was lost.
  const portal = await openCalibrate();
  const { fixture, dom } = portal;
  const fire = (target, type) =>
    act(async () =>
      target.dispatchEvent(new dom.window.Event(type, { bubbles: true })),
    );
  try {
    await press(button('Start'));
    const release = fixture.holdSaves();
    await fire(draftField('warmth'), 'pointerdown');
    await edit(draftField('warmth'), 25);
    await fire(dom.window, 'pointerup');
    await act(async () => sleep(5));
    assert.equal(button('Looks right').disabled, false, 'saving');
    await press(button('Looks right'));
    assert.equal(heading(), questions.red);
    await press(button('Back'));
    assert.equal(button('Reset').disabled, false);
    await press(button('Reset'));
    assert.equal(draftField('warmth').value, '0');

    // Cancel waits for the save on its way, then discards.
    await edit(draftField('tint'), 5);
    await press(button('Cancel calibration'));
    await press(button('Discard'));
    assert.equal(heading(), questions.white, 'waits for the save');
    assert.equal(button('Discard').getAttribute('aria-busy'), 'true');
    release();
    await act(async () => sleep(20));
    assert.equal(heading(), 'Calibration cancelled');
    assert.equal(processing(fixture).calibrations, undefined);
  } finally {
    await portal.unmount();
  }
});

test('without screen sync on the TV the page says what to do and shows no pattern', async () => {
  const portal = await openCalibrate({ target: 'off' });
  const { fixture } = portal;
  try {
    assert(text().includes('Open Prysm on the TV and choose Calibrate.'));
    assert.equal(button('Start').disabled, true);
    assert.deepEqual(patterns(fixture), []);
  } finally {
    await portal.unmount();
  }

  // The engine refuses a pattern without screen sync: nothing is created.
  const refused = await openCalibrate();
  try {
    refused.fixture.failNext({
      kind: 'not_processed',
      message: 'set the target to processed first',
    });
    await press(button('Start'));
    assert.equal(
      $('.calibrate [role="alert"]').textContent.includes(
        'Open Prysm on the TV and choose Calibrate.',
      ),
      true,
    );
    assert.equal(processing(refused.fixture).calibrations, undefined);
    assert.equal(heading(), 'Match the lights to your TV');
  } finally {
    await refused.unmount();
  }
});

test('calibration ending elsewhere keeps the draft and offers to resume', async () => {
  const portal = await openCalibrate();
  const { fixture } = portal;
  try {
    await press(button('Start'));
    await press(button('Looks right'));
    const [draft] = processing(fixture).calibrations;

    // The engine's 10-minute timeout, or the TV leaving the pattern screen.
    await act(async () => {
      fixture.publish({ calibration: undefined });
      await sleep(5);
    });
    assert.equal(heading(), 'Calibration ended');
    assert(text().includes('Your changes so far are kept in “Calibrated”.'));
    fixture.calls.length = 0;
    await press(button('Resume'));
    assert.deepEqual(patterns(fixture), ['calibration:red_pair']);
    assert.equal(heading(), questions.red);

    // Lights turned off: calibration ends and the TV is needed again.
    await act(async () => {
      await fixture.client.setTarget('off');
      await sleep(5);
    });
    assert.equal(heading(), 'Calibration ended');
    assert(text().includes('Open Prysm on the TV and choose Calibrate.'));
    await press(button('Leave'));
    assert(!$('.calibrate'));
    assert.deepEqual(processing(fixture).calibrations, [draft]);
    assert.equal(processing(fixture).active_calibration, draft.id);
  } finally {
    await portal.unmount();
  }
});

/** The next save is refused with `failure`; the TV's patterns still work. */
function failNextSave(fixture, failure) {
  const { client } = fixture;
  const replace = client.replaceSettings;
  client.replaceSettings = async () => {
    client.replaceSettings = replace;
    throw new ApplicationError(failure);
  };
}

test('a refused save at Start leaves no calibration, no pattern and no draft', async () => {
  // The pattern was shown and the draft kept: the TV stayed in calibration,
  // and a second Start saved two calibrations.
  const portal = await openCalibrate();
  const { fixture } = portal;
  try {
    failNextSave(fixture, { kind: 'unavailable', message: 'no answer' });
    await press(button('Start'));
    await settle();
    assert.equal(heading(), 'Match the lights to your TV');
    assert.equal(processing(fixture).calibrations, undefined);
    assert.deepEqual(patterns(fixture), [
      'calibration:gray_192',
      'calibration:end',
    ]);
    assert.equal('calibration' in fixture.snapshot(), false);
    assert.equal(
      $('.calibrate .notice.error').textContent,
      'Not saved: Prysm did not answer. Try again.',
    );
    assert.equal(button('Start').disabled, false);

    await press(button('Start'));
    await settle();
    assert.equal(heading(), questions.white);
    assert.equal(processing(fixture).calibrations.length, 1);
  } finally {
    await portal.unmount();
  }
});

test('a save elsewhere while the first pattern is on its way does not stop the start', async () => {
  // The draft was saved against the revision of the tap, refused as stale,
  // and saved later by itself, with no wizard on it.
  const portal = await openCalibrate({ calibrations: [ps5] });
  const { fixture } = portal;
  try {
    const release = fixture.holdPatterns();
    await act(async () => button('Start').click());
    // Another phone renames PS5 meanwhile.
    await act(async () => {
      const { settings, revision } = fixture.snapshot();
      await fixture.client.replaceSettings(revision, {
        ...settings,
        processing: {
          ...settings.processing,
          calibrations: [{ ...ps5, name: 'PS5 games' }],
        },
      });
      await sleep(0);
    });
    release();
    await act(async () => sleep(5));
    await settle();
    assert.equal(heading(), questions.white);
    const [renamed, draft] = processing(fixture).calibrations;
    assert.equal(renamed.name, 'PS5 games', 'the other change is kept');
    assert.equal(draft.name, 'Calibrated');
    assert.equal(processing(fixture).active_calibration, draft.id);
    assert.equal(processing(fixture).calibrations.length, 2);
  } finally {
    await portal.unmount();
  }
});

test('a refused Resume keeps calibration ended, with Resume and why', async () => {
  // The screen showed the pattern the TV did not show, without Resume.
  const portal = await openCalibrate();
  const { fixture } = portal;
  try {
    await press(button('Start'));
    await act(async () => {
      fixture.publish({ calibration: undefined });
      await sleep(5);
    });
    assert.equal(heading(), 'Calibration ended');
    fixture.failNext({ kind: 'unavailable', message: 'no answer' });
    await press(button('Resume'));
    assert.equal(heading(), 'Calibration ended');
    assert(button('Resume'));
    assert(text().includes('The TV pattern did not change: no answer'));
    await press(button('Resume'));
    assert.equal(heading(), questions.white, 'resumed');
  } finally {
    await portal.unmount();
  }
});

test('a failed slider save in the wizard is shown at the slider, with Try again', async () => {
  // The slider's place was not named: no retry was offered and nothing
  // saved until a slider moved again.
  const portal = await openCalibrate();
  const { fixture } = portal;
  try {
    await press(button('Start'));
    fixture.failNext({ kind: 'unavailable', message: 'no answer' });
    await edit(draftField('warmth'), 20);
    await settle();
    assert.equal(errorOf(draftField('warmth')), 'Not saved. Try again');
    await press(button('Try again'));
    await settle();
    assert.equal(processing(fixture).calibrations[0].correction.warmth, 20);
    assert.equal(errorOf(draftField('warmth')), undefined);
  } finally {
    await portal.unmount();
  }
});

test('a failed save at Finish keeps the wizard open with its buttons', async () => {
  const portal = await openCalibrate();
  const { fixture } = portal;
  try {
    await press(button('Start'));
    for (let screen = 1; screen < screens.length; screen++)
      await press(button('Looks right'));
    assert.equal(heading(), questions.review);
    await edit(field('calibration.name'), 'Movie night');
    failNextSave(fixture, { kind: 'unavailable', message: 'no answer' });
    await press(button('Finish'));
    await settle();
    assert.equal(heading(), questions.review, 'not finished');
    assert.equal(button('Finish').disabled, false);
    assert.equal(button('Finish').hasAttribute('aria-busy'), false);
    assert.notEqual(patterns(fixture).at(-1), 'calibration:end');
    await press(button('Finish'));
    await settle();
    assert.equal(heading(), 'Calibration finished');
    assert.equal(processing(fixture).calibrations[0].name, 'Movie night');
  } finally {
    await portal.unmount();
  }
});

/* ---- The wizard's state ---- */

const place = (screen, returnTo = null, uncorrected = false) => ({
  screen,
  returnTo,
  uncorrected,
});
const calibratingPhase = (change = {}) => ({
  kind: 'calibrating',
  draftId: 'd',
  previous: 'p',
  startedWith: zero,
  place: place(0),
  name: null,
  seen: true,
  request: { kind: 'idle' },
  confirming: false,
  closing: null,
  ...change,
});
const at = (phase, problem = '') => ({ phase, problem });

/** One state in each phase, and the ways a calibrating one can be. */
const states = {
  start: at({ kind: 'start', from: 'p' }, 'no answer'),
  starting: at({ kind: 'starting', from: 'p' }),
  calibrating: at(calibratingPhase()),
  sending: at(
    calibratingPhase({
      place: place(2),
      request: { kind: 'sending', confirmed: place(1), resuming: false },
    }),
  ),
  resuming: at(
    calibratingPhase({
      seen: false,
      request: { kind: 'sending', confirmed: place(0), resuming: true },
    }),
  ),
  confirming: at(calibratingPhase({ confirming: true })),
  closing: at(calibratingPhase({ closing: 'finish' })),
  ending: at({ kind: 'ending', last: calibratingPhase({ closing: 'finish' }) }),
  done: at({ kind: 'done', title: 'Calibration finished', message: 'Saved.' }),
};

/** One action of each kind. */
const actions = {
  sourceChosen: { type: 'sourceChosen', from: 'q' },
  startRequested: { type: 'startRequested' },
  startFailed: { type: 'startFailed', problem: 'no' },
  started: { type: 'started', draftId: 'd2', previous: 'p', startedWith: zero },
  patternSeen: { type: 'patternSeen' },
  requestSent: { type: 'requestSent', place: place(3, 4), resume: false },
  resumed: { type: 'requestSent', place: place(0), resume: true },
  patternShown: { type: 'patternShown' },
  patternShownMore: { type: 'patternShown', more: place(2) },
  requestFailed: { type: 'requestFailed', problem: 'no' },
  named: { type: 'named', name: 'Movie night' },
  confirmAsked: { type: 'confirmAsked' },
  kept: { type: 'kept' },
  closingStarted: { type: 'closingStarted', how: 'discard' },
  closingFailed: { type: 'closingFailed' },
  ending: { type: 'ending' },
  ended: { type: 'ended', title: 'Calibration cancelled', message: 'Deleted.' },
};

/**
 * What every calibrating phase does with a tap, a name and the end: the
 * screen moves at once and the TV follows, keeping the last place it
 * confirmed, and a resume waits for the TV to show a pattern again.
 */
function calibratingRow(phase) {
  const sending = phase.request.kind === 'sending' ? phase.request : null;
  const confirmed = sending ? sending.confirmed : phase.place;
  return {
    requestSent: at({
      ...phase,
      place: place(3, 4),
      request: {
        kind: 'sending',
        confirmed,
        resuming: sending ? sending.resuming : false,
      },
    }),
    resumed: at({
      ...phase,
      place: place(0),
      seen: false,
      request: { kind: 'sending', confirmed, resuming: true },
    }),
    named: at({ ...phase, name: 'Movie night' }),
    ending: at({ kind: 'ending', last: phase }),
  };
}

/** Every change each action makes in each state; any other pair changes nothing. */
const transitions = {
  start: {
    sourceChosen: at({ kind: 'start', from: 'q' }, 'no answer'),
    startRequested: at({ kind: 'starting', from: 'p' }),
  },
  starting: {
    startFailed: at({ kind: 'start', from: 'p' }, 'no'),
    started: at(calibratingPhase({ draftId: 'd2', seen: false })),
  },
  calibrating: {
    ...calibratingRow(states.calibrating.phase),
    confirmAsked: at(calibratingPhase({ confirming: true })),
    closingStarted: at(calibratingPhase({ closing: 'discard' })),
  },
  sending: {
    ...calibratingRow(states.sending.phase),
    patternShown: at({ ...states.sending.phase, request: { kind: 'idle' } }),
    patternShownMore: at({
      ...states.sending.phase,
      request: { kind: 'sending', confirmed: place(2), resuming: false },
    }),
    requestFailed: at(
      { ...states.sending.phase, place: place(1), request: { kind: 'idle' } },
      'no',
    ),
    confirmAsked: at({ ...states.sending.phase, confirming: true }),
    closingStarted: at({ ...states.sending.phase, closing: 'discard' }),
  },
  resuming: {
    ...calibratingRow(states.resuming.phase),
    patternSeen: at({ ...states.resuming.phase, seen: true }),
    patternShown: at({ ...states.resuming.phase, request: { kind: 'idle' } }),
    patternShownMore: at({
      ...states.resuming.phase,
      request: { kind: 'sending', confirmed: place(2), resuming: true },
    }),
    // Refused: calibration is still ended, so the ended screen shows again.
    requestFailed: at(
      { ...states.resuming.phase, seen: true, request: { kind: 'idle' } },
      'no',
    ),
    confirmAsked: at({ ...states.resuming.phase, confirming: true }),
    closingStarted: at({ ...states.resuming.phase, closing: 'discard' }),
  },
  confirming: {
    ...calibratingRow(states.confirming.phase),
    kept: at(calibratingPhase()),
    closingStarted: at(
      calibratingPhase({ confirming: true, closing: 'discard' }),
    ),
  },
  closing: {
    ...calibratingRow(states.closing.phase),
    closingFailed: at(calibratingPhase()),
  },
  ending: {
    ended: at({
      kind: 'done',
      title: 'Calibration cancelled',
      message: 'Deleted.',
    }),
  },
  done: {},
};

test('the wizard: each action in each state, and no change anywhere else', () => {
  for (const [name, state] of Object.entries(states))
    for (const [kind, action] of Object.entries(actions)) {
      const next = wizardReducer(state, action);
      const wanted = transitions[name][kind];
      if (wanted) assert.deepEqual(next, wanted, `${name} × ${kind}`);
      else assert.equal(next, state, `${name} × ${kind} changes nothing`);
    }
  assert.deepEqual(initialWizard, at({ kind: 'start', from: '' }));
});

test('a tap moves to a place: a new screen forgets the check unless it names one', () => {
  const fromCheck = place(2, 4, true);
  assert.deepEqual(placeAfter(fromCheck, { screen: 3 }), place(3));
  assert.deepEqual(
    placeAfter(fromCheck, { screen: 1, returnTo: 5 }),
    place(1, 5),
  );
  // Comparing, or showing the screen again, keeps the check and ends the comparison.
  assert.deepEqual(
    placeAfter(fromCheck, { uncorrected: true }),
    place(2, 4, true),
  );
  assert.deepEqual(placeAfter(fromCheck, {}), place(2, 4));
});

test('Cancel asks only when a slider moved or a name was typed', () => {
  const phase = calibratingPhase({ startedWith: zero });
  const draft = { id: 'd', name: 'Calibrated', correction: zero };
  assert.equal(losesWork(phase, draft), false);
  assert.equal(losesWork(phase, undefined), false, 'deleted elsewhere');
  assert.equal(
    losesWork(phase, { ...draft, correction: { ...zero, warmth: 3 } }),
    true,
  );
  assert.equal(losesWork({ ...phase, name: ' Calibrated ' }, draft), false);
  assert.equal(losesWork({ ...phase, name: 'Movie night' }, draft), true);
});

test('the screens: white, one per primary on its pair, checks and the review', () => {
  assert.deepEqual(
    screens.map(({ section, pattern }) => `${section}:${pattern}`),
    [
      'white:gray_192',
      'red:red_pair',
      'green:green_pair',
      'blue:blue_pair',
      'checks:yellow',
      'checks:cyan',
      'checks:magenta',
      'checks:coast',
      'checks:garden',
      'checks:dusk',
      'review:review_grid',
    ],
  );
  assert.deepEqual(
    screens.filter((screen) => screen.fixes).map((screen) => screen.fixes),
    [
      ['red', 'green'],
      ['green', 'blue'],
      ['red', 'blue'],
      ['white', 'red', 'green', 'blue'],
      ['white', 'red', 'green', 'blue'],
      ['white', 'red', 'green', 'blue'],
    ],
  );
  assert.equal(screens[screenOf('green')].pattern, 'green_pair');
});
