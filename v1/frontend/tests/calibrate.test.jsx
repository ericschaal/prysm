import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import { WebosWebPortal } from '../hosts/webos/web-portal/app';
import { calibrateRoute, processingDelayMs } from '@prysm/web-portal';
import {
  initialWizard,
  screens,
  wizardReducer,
} from '../packages/web-portal/calibrate-wizard';
import {
  $,
  button,
  edit,
  field,
  openSettings,
  renderPortal,
  sleep,
  text,
} from './dom';

const capture = { kind: 'device', path: '/dev/video0' };
const output = {
  kind: 'wled',
  address: '192.0.2.1',
  leds: { top: 10, left: 4 },
};
const zero = {
  warmth: 0,
  tint: 0,
  midtones: 0,
  intensity: 0,
  hue: { red: 0, yellow: 0, green: 0, cyan: 0, blue: 0, magenta: 0 },
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

/** Screen sync running, optionally with saved profiles; opens the Calibrate page. */
async function openCalibrate({
  target = 'processed',
  profiles,
  Host = HeadlessWebPortal,
} = {}) {
  const portal = await renderPortal(Host, { capture, output }, (snapshot) => ({
    target,
    session: { state: target === 'processed' ? 'running' : 'stopped' },
    settings: {
      ...snapshot.settings,
      processing: {
        ...snapshot.settings.processing,
        ...(profiles ? { profiles, active_profile: profiles[0].id } : {}),
      },
    },
  }));
  await openSettings();
  await press(button('Calibrate colours'));
  return portal;
}

test('Calibrate opens from the portal at #/calibrate on both hosts', async () => {
  for (const Host of [HeadlessWebPortal, WebosWebPortal]) {
    const portal = await openCalibrate({ Host });
    try {
      assert.equal(window.location.hash, calibrateRoute);
      assert.equal(heading(), 'Match the lights to your TV');
      assert.equal(document.activeElement, $('.calibrate h2'));
      assert(text().includes('calibrate in the mode you watch in'));
      assert.equal(
        field('calibration.from'),
        null,
        'no question without profiles',
      );
      await press(button('Leave'));
      assert.equal($('.calibrate'), null);
      assert(button('Calibrate colours'), 'back in Settings');
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
      window.location.hash = calibrateRoute;
      await sleep(0);
    });
    assert.equal(heading(), 'Match the lights to your TV');
    await press(button('Leave'));
    assert.equal($('.calibrate'), null);
    assert.equal(window.location.hash, '');
    assert($('.light-card'), 'Home');
  } finally {
    await portal.unmount();
  }
});

test('a new profile is calibrated step by step and finished under its name', async () => {
  const portal = await openCalibrate();
  const { fixture } = portal;
  try {
    await press(button('Start'));
    const [draft] = processing(fixture).profiles;
    assert.deepEqual(draft, {
      id: draft.id,
      name: 'Calibrated',
      correction: zero,
    });
    assert.equal(processing(fixture).active_profile, draft.id);
    assert.deepEqual(patterns(fixture), ['calibration:gray-192']);
    assert.deepEqual(fixture.snapshot().calibration, {
      pattern: 'gray-192',
      uncorrected: false,
    });
    assert.equal(heading(), 'Find a natural white');
    assert.equal($('.calibrate-pattern').textContent, 'On your TV: light gray');

    // Slider edits are live saves on the draft profile.
    await edit(field('processing.profiles[0].correction.warmth'), 20);
    await edit(field('processing.profiles[0].correction.tint'), -5);
    await settle();
    assert.deepEqual(processing(fixture).profiles[0].correction, {
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

    fixture.calls.length = 0;
    await press(button('Next'));
    assert.equal(heading(), 'Fine-tune each colour');
    assert.equal(
      $('.calibrate-pattern').textContent,
      'On your TV: red (1 of 6)',
    );
    await edit(field('processing.profiles[0].correction.hue.red'), 8);
    await settle();
    assert.equal(processing(fixture).profiles[0].correction.hue.red, 8);
    for (let step = 0; step < 12; step++) await press(button('Next'));
    assert.equal(heading(), 'Name your profile');
    assert.equal(button('Next'), undefined);
    await press(button('Back'));
    assert.equal(heading(), 'Check colours together');
    await press(button('Next'));
    assert.deepEqual(patterns(fixture), [
      'calibration:red',
      'calibration:yellow',
      'calibration:green',
      'calibration:cyan',
      'calibration:blue-pair',
      'calibration:magenta',
      'calibration:red-pair',
      'calibration:green-pair',
      'calibration:blue-pair',
      'calibration:coast',
      'calibration:garden',
      'calibration:dusk',
      'calibration:review-grid',
      'calibration:dusk',
      'calibration:review-grid',
    ]);

    const name = field('calibration.name');
    assert.equal(name.value, 'Calibrated');
    await edit(name, '  Movie night ');
    await press(button('Finish'));
    await act(async () => sleep(5));
    assert.deepEqual(processing(fixture).profiles, [
      {
        id: draft.id,
        name: 'Movie night',
        correction: {
          ...zero,
          warmth: 20,
          tint: -5,
          hue: { ...zero.hue, red: 8 },
        },
      },
    ]);
    assert.equal(processing(fixture).active_profile, draft.id);
    assert.equal(patterns(fixture).at(-1), 'calibration:end');
    assert.equal('calibration' in fixture.snapshot(), false);
    assert.equal(heading(), 'Calibration finished');
    assert(text().includes('“Movie night” is saved and active.'));
    await press(button('Back to Prysm'));
    assert.equal($('.calibrate'), null);
  } finally {
    await portal.unmount();
  }
});

test('starting from a profile calibrates a copy; Cancel deletes it and restores the previous one', async () => {
  const portal = await openCalibrate({ profiles: [ps5] });
  const { fixture } = portal;
  try {
    await edit(field('calibration.from'), 'ps5');
    await press(button('Start'));
    const [, copy] = processing(fixture).profiles;
    assert.notEqual(copy.id, 'ps5');
    assert.deepEqual(copy, {
      id: copy.id,
      name: 'PS5 calibrated',
      correction: ps5.correction,
    });
    assert.equal(processing(fixture).active_profile, copy.id);
    assert.deepEqual(patterns(fixture), ['calibration:gray-192']);

    await press(button('Next'));
    await press(button('Cancel calibration'));
    await act(async () => sleep(5));
    assert.deepEqual(processing(fixture).profiles, [ps5]);
    assert.equal(processing(fixture).active_profile, 'ps5');
    assert.equal(patterns(fixture).at(-1), 'calibration:end');
    assert.equal('calibration' in fixture.snapshot(), false);
    assert.equal(heading(), 'Calibration cancelled');
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
    assert.equal(processing(refused.fixture).profiles, undefined);
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
    await press(button('Next'));
    const [draft] = processing(fixture).profiles;

    // The engine's 10-minute timeout, or the TV leaving the pattern screen.
    await act(async () => {
      fixture.publish({ calibration: undefined });
      await sleep(5);
    });
    assert.equal(heading(), 'Calibration ended');
    assert(text().includes('Your changes so far are kept in “Calibrated”.'));
    fixture.calls.length = 0;
    await press(button('Resume'));
    assert.deepEqual(patterns(fixture), ['calibration:red']);
    assert.equal(heading(), 'Fine-tune each colour');

    // Lights turned off: calibration ends and the TV is needed again.
    await act(async () => {
      await fixture.client.setTarget('off');
      await sleep(5);
    });
    assert.equal(heading(), 'Calibration ended');
    assert(text().includes('Open Prysm on the TV and choose Calibrate.'));
    await press(button('Leave'));
    assert.equal($('.calibrate'), null);
    assert.deepEqual(processing(fixture).profiles, [draft]);
    assert.equal(processing(fixture).active_profile, draft.id);
  } finally {
    await portal.unmount();
  }
});

test('the wizard moves only on what the TV confirms and ends once', () => {
  const run = (...actions) => actions.reduce(wizardReducer, initialWizard);
  // Before the start, a pattern does not move any screen.
  let state = run(
    { type: 'sourceChosen', from: 'p1' },
    { type: 'requestSent' },
    { type: 'patternShown', screen: 3 },
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
    name: null,
  });
  assert.equal(state.seen, false, 'waits for the TV to show the pattern');
  state = wizardReducer(state, { type: 'patternSeen' });

  // Next: busy while the TV changes, the screen moves when it did.
  state = wizardReducer(state, { type: 'requestSent' });
  assert.equal(state.busy, true);
  assert.equal(state.phase.screen, 0);
  state = wizardReducer(state, { type: 'patternShown', screen: 1 });
  assert.equal(state.busy, false);
  assert.equal(state.phase.screen, 1);

  // A refused pattern keeps the screen and says why; the next request clears it.
  state = [
    { type: 'started', draftId: 'd', previous: undefined },
    { type: 'requestSent' },
    { type: 'requestFailed', problem: 'no' },
  ].reduce(wizardReducer, initialWizard);
  assert.equal(state.phase.screen, 0);
  assert.equal(state.problem, 'no');
  assert.equal(wizardReducer(state, { type: 'requestSent' }).problem, '');

  // Typing a name clears a name problem; the last screen is the review.
  state = [
    { type: 'refused', problem: 'Enter a name.' },
    { type: 'named', name: 'Movie night' },
  ].reduce(wizardReducer, state);
  assert.equal(state.problem, '');
  assert.equal(state.phase.name, 'Movie night');
  assert.equal(screens.at(-1).section, 'review');

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
