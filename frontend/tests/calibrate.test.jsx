import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import { processingDelayMs } from '@prysm/web-portal';
import {
  initialWizard,
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
    assert.deepEqual(patterns(fixture), ['calibration:gray-192']);
    assert.deepEqual(fixture.snapshot().calibration, {
      pattern: 'gray-192',
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
      'calibration:gray-192:uncorrected',
      'calibration:gray-192',
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
      'calibration:red-pair',
      'calibration:green-pair',
      'calibration:blue-pair',
      'calibration:yellow',
      'calibration:green-pair',
      'calibration:yellow',
      'calibration:cyan',
      'calibration:magenta',
      'calibration:coast',
      'calibration:gray-192',
      'calibration:coast',
      'calibration:garden',
      'calibration:dusk',
      'calibration:review-grid',
      'calibration:dusk',
      'calibration:review-grid',
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
    assert.deepEqual(patterns(fixture), ['calibration:gray-192']);

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
    assert.deepEqual(patterns(fixture), ['calibration:gray-192']);

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
      'calibration:red-pair',
      'calibration:green-pair',
    ]);
    assert.equal(fixture.snapshot().calibration.pattern, 'green-pair');

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
    assert.deepEqual(patterns(fixture), ['calibration:red-pair']);
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

test('the wizard moves at once, the TV follows, a refusal goes back, and it ends once', () => {
  // It used to move only once the TV confirmed, so a tap showed nothing for
  // a whole request. Now it moves at once and goes back to what the TV shows
  // if the TV refuses: the screen still never stays on a pattern the TV lacks.
  const run = (...actions) => actions.reduce(wizardReducer, initialWizard);
  // Before the start, a pattern does not move any screen.
  let state = run(
    { type: 'sourceChosen', from: 'p1' },
    { type: 'requestSent', screen: 3 },
    { type: 'patternShown' },
  );
  assert.deepEqual(state.phase, { kind: 'start', from: 'p1' });
  assert.equal(state.busy, false);

  state = run(
    { type: 'patternRequested' },
    { type: 'requestSent' },
    { type: 'patternShown' },
    { type: 'started', draftId: 'd', previous: 'p1' },
  );
  assert.deepEqual(state.phase, {
    kind: 'calibrating',
    draftId: 'd',
    previous: 'p1',
    screen: 0,
    returnTo: null,
    name: null,
  });
  assert.equal(state.seen, false, 'waits for the TV to show the pattern');
  state = wizardReducer(state, { type: 'patternSeen' });

  // Next: the screen moves at once, busy until the TV shows its pattern.
  state = wizardReducer(state, { type: 'requestSent', screen: 1 });
  assert.equal(state.busy, true);
  assert.equal(state.phase.screen, 1);
  assert.deepEqual(state.confirmed, {
    screen: 0,
    returnTo: null,
    uncorrected: false,
  });
  state = wizardReducer(state, { type: 'patternShown' });
  assert.equal(state.busy, false);
  assert.equal(state.phase.screen, 1);
  assert.equal(state.confirmed, null);

  // From a check to a colour and back: the check is kept while the colour
  // shows, also through a comparison or a resume, and cleared on the way back.
  // The comparison shows at once; a move ends it.
  const yellow = screens.findIndex((screen) => screen.pattern === 'yellow');
  state = [
    { type: 'requestSent', screen: screenOf('green'), returnTo: yellow },
    { type: 'patternShown' },
  ].reduce(wizardReducer, state);
  assert.equal(state.phase.screen, screenOf('green'));
  assert.equal(state.phase.returnTo, yellow);
  state = wizardReducer(state, { type: 'requestSent', uncorrected: true });
  assert.equal(state.uncorrected, true);
  state = [{ type: 'patternShown' }, { type: 'requestSent' }].reduce(
    wizardReducer,
    state,
  );
  assert.equal(state.phase.returnTo, yellow);
  state = [
    { type: 'patternShown' },
    { type: 'requestSent', uncorrected: true },
    { type: 'patternShown' },
    { type: 'requestSent', screen: yellow },
    { type: 'patternShown' },
  ].reduce(wizardReducer, state);
  assert.equal(state.phase.screen, yellow);
  assert.equal(state.phase.returnTo, null);
  assert.equal(state.uncorrected, false);

  // Two quick taps: the first answer is not the last, so it stays busy and
  // keeps what the TV shows; the newest refused goes back there and says why.
  state = [
    { type: 'requestSent', screen: yellow + 1 },
    { type: 'requestSent', screen: yellow + 2 },
  ].reduce(wizardReducer, state);
  assert.equal(state.phase.screen, yellow + 2);
  assert.equal(state.confirmed.screen, yellow);
  state = wizardReducer(state, {
    type: 'patternShown',
    more: { screen: yellow + 1, returnTo: null, uncorrected: false },
  });
  assert.equal(state.busy, true, 'the newest is still on its way');
  assert.equal(state.phase.screen, yellow + 2);
  state = wizardReducer(state, { type: 'requestFailed', problem: 'no' });
  assert.equal(state.busy, false);
  assert.equal(state.phase.screen, yellow + 1, 'what the TV shows');
  assert.equal(state.problem, 'no');
  // A refused comparison goes back too.
  state = [
    { type: 'requestSent', uncorrected: true },
    { type: 'requestFailed', problem: 'no' },
  ].reduce(wizardReducer, state);
  assert.equal(state.uncorrected, false);
  assert.equal(state.phase.screen, yellow + 1);

  // A refused pattern keeps the screen and says why; the next request clears it.
  state = [
    { type: 'started', draftId: 'd', previous: undefined },
    { type: 'requestSent' },
    { type: 'requestFailed', problem: 'no' },
  ].reduce(wizardReducer, initialWizard);
  assert.equal(state.phase.screen, 0);
  assert.equal(state.problem, 'no');
  assert.equal(wizardReducer(state, { type: 'requestSent' }).problem, '');

  // Typing a name clears the problem; the last screen is the review.
  state = wizardReducer(state, { type: 'named', name: 'Movie night' });
  assert.equal(state.problem, '');
  assert.equal(state.phase.name, 'Movie night');

  // White, one screen per primary on its pair, checks, review.
  assert.deepEqual(
    screens.map(({ section, pattern }) => `${section}:${pattern}`),
    [
      'white:gray-192',
      'red:red-pair',
      'green:green-pair',
      'blue:blue-pair',
      'checks:yellow',
      'checks:cyan',
      'checks:magenta',
      'checks:coast',
      'checks:garden',
      'checks:dusk',
      'review:review-grid',
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

  state = [
    { type: 'ending' },
    { type: 'ended', title: 'Calibration finished', message: 'Saved.' },
  ].reduce(wizardReducer, state);
  assert.deepEqual(state.phase, {
    kind: 'done',
    title: 'Calibration finished',
    message: 'Saved.',
  });
  assert.equal(state.busy, false);
  assert.equal(wizardReducer(state, { type: 'named', name: 'x' }), state);
});
