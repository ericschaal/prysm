/**
 * What the lights are doing, decided once (`lightingSituation`), its words,
 * the light card's view of it, and the card on the portal.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { act } from 'react';
import {
  describeOutputIssue,
  describeSituation,
  lightingSituation,
  plainFailure,
  situationFailure,
} from '@prysm/application-client';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import {
  lightCardView,
  moodOf,
  pauseOf,
} from '../packages/web-portal/light-card-view';
import { $, navigated, renderPortal } from './dom';

const failure = { kind: 'adapter', message: 'WLED cannot be reached' };
const invalid = {
  kind: 'invalid',
  field: 'output.address',
  message: 'a unicast device address is required',
};
const message = 'The video source did not close in time.';
/** A session as the portal reads it; `failed` puts `failure` in it. */
const session = (state, failed = false) => {
  if (state === 'faulted') return { state, message };
  if (failed) return { state, failure };
  return { state };
};

test('every target and session has one lighting situation', () => {
  // target, session, the session carries a failure, situation
  const targets = ['off', 'screen_sync', 'effects', 'unknown'];
  const sessions = [
    ['stopped', false],
    ['stopped', true],
    ['screen_sync', false],
    ['effects', false],
    ['effects', true],
    ['faulted', false],
    ['unknown', false],
    ['unknown', true],
  ];
  const expected = {
    off: ['off', 'off', 'off', 'off', 'off', 'faulted', 'unknown', 'unknown'],
    screen_sync: [
      'starting',
      'failed',
      'sync',
      'starting',
      'failed',
      'faulted',
      'unknown',
      'unknown',
    ],
    effects: [
      'starting',
      'failed',
      'starting',
      'effects',
      'effects',
      'faulted',
      'unknown',
      'unknown',
    ],
    unknown: [
      'unknown',
      'unknown',
      'unknown',
      'unknown',
      'unknown',
      'faulted',
      'unknown',
      'unknown',
    ],
  };
  for (const target of targets)
    sessions.forEach(([state, failed], index) => {
      const shown = `${target} ${state}${failed ? ' with a failure' : ''}`;
      const situation = lightingSituation({
        target,
        session: session(state, failed),
      });
      assert.equal(situation.kind, expected[target][index], shown);
      // A failure in the session is never dropped, except while switching
      // off from the effects (the effects' failure is no longer current)
      // and while faulted (the fault says what to do).
      const dropped = target === 'off' && state === 'effects';
      assert.equal(
        situationFailure(situation),
        failed && !dropped ? failure : null,
        `${shown} keeps its failure`,
      );
    });
  const input = { fps: 30, receiving: true };
  assert.deepEqual(
    lightingSituation({
      target: 'screen_sync',
      session: session('screen_sync'),
      input,
    }),
    { kind: 'sync', input },
  );
  assert.deepEqual(
    lightingSituation({
      target: 'screen_sync',
      session: session('screen_sync'),
    }),
    { kind: 'sync', input: undefined },
    'not measured yet is not "not receiving"',
  );
  assert.deepEqual(
    lightingSituation({ target: 'effects', session: session('stopped') }),
    { kind: 'starting', target: 'effects' },
  );
  assert.deepEqual(
    lightingSituation({ target: 'screen_sync', session: session('faulted') }),
    { kind: 'faulted', message },
  );
  // A state newer than this page is decoded as `unknown` (http.test.jsx).
  assert.deepEqual(
    lightingSituation({
      target: 'screen_sync',
      session: { state: 'unknown', failure, message: 'newer' },
    }),
    { kind: 'unknown', failure, message: 'newer' },
  );
  // A start of screen sync that failed from the lights' own effects.
  assert.equal(
    lightingSituation({
      target: 'screen_sync',
      session: session('effects', true),
    }).effectsOn,
    true,
  );
  // The side that failed: typed, else the setting named.
  const at = (change) =>
    lightingSituation({
      target: 'screen_sync',
      session: { state: 'stopped', failure: { ...failure, ...change } },
    }).at;
  assert.equal(at({}), null);
  assert.equal(at({ section: 'capture' }), 'capture');
  assert.equal(at({ kind: 'invalid', field: 'output.leds' }), 'output');
  assert.equal(at({ kind: 'invalid', field: 'processing.style' }), null);
});

test('each situation has one headline and one sentence', () => {
  const words = (target, state, failed = false) =>
    describeSituation(
      lightingSituation({ target, session: session(state, failed) }),
      'WLED effects',
    );
  assert.deepEqual(words('off', 'screen_sync'), {
    headline: 'Lights are off',
    sentence: 'Lights are off.',
  });
  assert.deepEqual(words('screen_sync', 'screen_sync'), {
    headline: 'Lights follow your screen',
    sentence: 'Screen sync is running.',
  });
  assert.deepEqual(words('effects', 'effects'), {
    headline: 'WLED effects are on',
    sentence: 'WLED effects are on.',
  });
  assert.deepEqual(words('effects', 'effects', true), {
    headline: 'WLED effects are on',
    sentence: 'WLED effects are on.',
  });
  assert.deepEqual(words('screen_sync', 'effects'), {
    headline: 'Starting screen sync…',
    sentence: 'Starting screen sync…',
  });
  assert.deepEqual(words('effects', 'screen_sync'), {
    headline: 'Starting WLED effects…',
    sentence: 'Starting WLED effects…',
  });
  assert.deepEqual(words('screen_sync', 'stopped', true), {
    headline: 'Screen sync stopped',
    sentence: 'Screen sync stopped.',
  });
  assert.deepEqual(words('screen_sync', 'effects', true), {
    headline: 'Screen sync could not start',
    sentence: 'Screen sync could not start.',
  });
  assert.deepEqual(words('effects', 'stopped', true), {
    headline: 'WLED effects could not start',
    sentence: 'WLED effects could not start.',
  });
  assert.deepEqual(words('screen_sync', 'faulted'), {
    headline: 'Prysm stopped responding',
    sentence: 'Prysm stopped responding. Restart Prysm to recover.',
  });
  assert.deepEqual(words('unknown', 'stopped'), {
    headline: 'Prysm is newer than this app',
    sentence:
      'Prysm is newer than this app. Update it to see what the lights do.',
  });
  assert.equal(
    describeOutputIssue('sleeping'),
    'The lights are not showing screen sync.',
  );
});

const facts = (lighting, change = {}) => ({
  target: 'screen_sync',
  lighting,
  device: {},
  hasCapture: true,
  hasOutput: true,
  output: { effects: 'WLED effects' },
  ...change,
});
const memory = (change = {}) => ({
  remembered: 'screen_sync',
  error: null,
  lit: false,
  note: null,
  ...change,
});

test('the light card view: mood, word, line and mode from the situation', () => {
  const sync = { kind: 'sync', input: { fps: 30, receiving: true } };
  const failed = {
    kind: 'failed',
    target: 'screen_sync',
    failure,
    at: 'output',
    effectsOn: false,
  };
  assert.equal(moodOf(sync, true), 'sync');
  assert.equal(moodOf(failed, true), 'problem');
  assert.equal(moodOf({ kind: 'faulted', message }, true), 'problem');
  const newer = { kind: 'unknown', failure: null, message: null };
  assert.equal(moodOf(newer, true), 'unknown');
  const unknown = lightCardView(facts(newer, { target: 'unknown' }), memory());
  assert.deepEqual(
    [unknown.word, unknown.on, unknown.status, unknown.info],
    [
      'Unknown',
      false,
      'Prysm is newer than this app',
      {
        tone: 'note',
        text: 'Update it to see what the lights do.',
        details: undefined,
      },
    ],
  );
  const faulted = lightCardView(facts({ kind: 'faulted', message }), memory());
  assert.deepEqual(
    [faulted.word, faulted.status, faulted.info],
    [
      'Stopped',
      'Prysm stopped responding',
      {
        tone: 'problem',
        text: 'Restart Prysm to recover.',
        details: message,
        retry: false,
      },
    ],
  );
  // Effects on with saved settings that do not validate: a note.
  const effectsInvalid = lightCardView(
    facts({ kind: 'effects', failure: invalid }, { target: 'effects' }),
    memory({ lit: true }),
  );
  assert.deepEqual(effectsInvalid.info, {
    tone: 'note',
    text: 'Prysm can’t use these settings.',
    details: invalid.message,
  });
  assert.equal(
    moodOf({ kind: 'starting', target: 'screen_sync' }, true),
    'starting',
  );
  assert.equal(moodOf(sync, false), 'unset');

  assert.equal(
    pauseOf({ kind: 'sync', input: { fps: 0, receiving: false } }, true),
    'waiting',
  );
  assert.equal(pauseOf({ kind: 'sync', input: undefined }, true), null);
  assert.equal(
    pauseOf({ kind: 'starting', target: 'screen_sync' }, false),
    'starting',
  );
  assert.equal(
    pauseOf({ kind: 'starting', target: 'screen_sync' }, true),
    null,
  );

  const running = lightCardView(facts(sync), memory({ lit: true }));
  assert.deepEqual(
    [running.word, running.palette, running.calm, running.info, running.status],
    ['On', 'sync', 'Following the screen.', null, 'Lights follow your screen'],
  );
  const stopped = lightCardView(facts(failed), memory());
  assert.deepEqual(
    [stopped.word, stopped.palette, stopped.calm],
    ['Stopped', 'error', ''],
  );
  assert.deepEqual(stopped.info, {
    tone: 'problem',
    text: 'Screen sync could not start.',
    details: 'WLED cannot be reached',
    retry: true,
  });
  // A refused click with nothing failed in the state: said, without Try again.
  const refused = lightCardView(
    facts({ kind: 'off', failure: null }, { target: 'off' }),
    memory({
      error: {
        failure: { kind: 'busy', message: 'busy' },
        asked: 'screen_sync',
      },
    }),
  );
  assert.equal(refused.info.text, 'Prysm is busy. Try again in a moment.');
  assert.equal(refused.info.retry, false);
  // Off: the chosen mode is the remembered one, if it can run.
  const off = lightCardView(
    facts({ kind: 'off', failure: null }, { target: 'off' }),
    memory({ remembered: 'effects' }),
  );
  assert.deepEqual(
    [off.mode, off.calm],
    ['effects', 'Power turns on WLED effects.'],
  );
  assert.equal(off.ownEffects, true);
  const noEffects = lightCardView(
    facts({ kind: 'off', failure: null }, { target: 'off', output: {} }),
    memory({ remembered: 'effects' }),
  );
  assert.equal(noEffects.mode, 'screen_sync');
  assert.equal(noEffects.calm, 'Power turns on Screen sync.');
  // An output issue is said only while screen sync runs.
  const issue = { device: { issue: 'other_sender' } };
  assert.equal(
    lightCardView(facts(sync, issue), memory({ lit: true })).info.tone,
    'warning',
  );
  assert.equal(
    lightCardView(
      facts({ kind: 'off', failure: null }, { ...issue, target: 'off' }),
      memory(),
    ).info,
    null,
  );
});

const capture = { kind: 'device', path: '/dev/video0' };
const output = {
  kind: 'wled',
  address: '192.0.2.1',
  leds: { top: 10, left: 4 },
};
const press = (element) =>
  act(async () => {
    element.click();
    await navigated();
  });
const shown = () => $('.light-info').dataset.shown === 'true';

test('a refused click is said only until the lights change', async () => {
  // It stayed on screen, above the live state, until the next click.
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    fixture.failNext({
      kind: 'unavailable',
      message: 'The portal did not answer.',
    });
    await press($('[data-power]'));
    assert(shown());
    assert.equal(
      $('.light-info > div > p').textContent,
      'A device is not available right now.',
    );
    // The command ran after all: the state says so, and the error goes.
    await act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: { state: 'screen_sync' },
      }),
    );
    assert.equal(shown(), false);
    // A refused click while it runs stays while nothing changes.
    fixture.failNext({ kind: 'busy', message: 'busy' });
    await press($('[data-target="effects"]'));
    assert(shown());
    assert.equal(
      $('.light-info > div > p').textContent,
      'Prysm is busy. Try again in a moment.',
    );
    // Changed on another device: the old error goes too.
    await act(async () =>
      fixture.publish({ target: 'off', session: { state: 'stopped' } }),
    );
    assert.equal(shown(), false);
  } finally {
    await portal.unmount();
  }
});

test('an old refused click does not come back when the lights do', async () => {
  // Hidden only while the lights differed: back as they were, it showed again.
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    fixture.failNext({ kind: 'busy', message: 'busy' });
    await press($('[data-power]'));
    assert(shown());
    await act(async () =>
      fixture.publish({
        target: 'screen_sync',
        session: { state: 'screen_sync' },
      }),
    );
    await act(async () =>
      fixture.publish({ target: 'off', session: { state: 'stopped' } }),
    );
    assert.equal(shown(), false);
  } finally {
    await portal.unmount();
  }
});

test('with the lights off a failure is still said, as off', () => {
  // Settings that do not apply at the start, or a switch off that failed:
  // the failure was dropped, and nothing said why.
  const lost = { kind: 'adapter', section: 'output', message: 'no answer' };
  const off = lightingSituation({
    target: 'off',
    session: { state: 'stopped', failure: lost },
  });
  assert.deepEqual(off, { kind: 'off', failure: lost });
  assert.equal(
    describeSituation(off, 'WLED effects').headline,
    'Lights are off',
  );
  assert.equal(
    plainFailure(lost, 'off', 'WLED effects'),
    'The lights are not responding.',
  );
  assert.equal(
    plainFailure({ kind: 'adapter', message: 'x' }, 'off', 'WLED effects'),
    'The lights could not be switched off.',
  );
  assert.equal(
    plainFailure({ kind: 'invalid', message: 'x' }, 'off', 'WLED effects'),
    'Prysm can’t use these settings.',
  );
  // As the engine reports saved settings that do not validate: the field
  // names its section, and no device was asked.
  assert.equal(
    plainFailure(
      { kind: 'invalid', message: 'x', field: 'output.address' },
      'off',
      'WLED effects',
    ),
    'Prysm can’t use these settings.',
  );
  // The card: off, with the problem and its details, and no Try again (it
  // could only switch off again).
  const view = lightCardView(facts(off, { target: 'off' }), memory());
  assert.deepEqual(
    [view.word, view.calm, view.status],
    ['Off', '', 'Lights are off'],
  );
  assert.deepEqual(view.info, {
    tone: 'problem',
    text: 'The lights are not responding.',
    details: 'no answer',
    retry: false,
  });
  // A refused click is said by what it asked for.
  const refused = lightCardView(
    facts({ kind: 'off', failure: null }, { target: 'off' }),
    memory({
      error: {
        failure: { kind: 'adapter', message: 'x' },
        asked: 'screen_sync',
      },
    }),
  );
  assert.equal(refused.info.text, 'Screen sync could not start.');
});

test('the light card shows a failure with the lights off, without Try again', async () => {
  // It was hidden while the target was off.
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    await act(async () =>
      fixture.publish({
        session: {
          state: 'stopped',
          failure: { kind: 'adapter', section: 'output', message: 'no answer' },
        },
      }),
    );
    assert.equal(fixture.snapshot().target, 'off');
    assert(shown());
    assert.equal(
      $('.light-info > div > p').textContent,
      'The lights are not responding.',
    );
    assert.equal($('.light-word [data-shown="true"]').textContent, 'Off');
    assert.equal($('.lighting-status').textContent, 'Lights are off');
    assert(
      ![...document.querySelectorAll('.light-card button')].some(
        (item) => item.textContent === 'Try again',
      ),
    );
  } finally {
    await portal.unmount();
  }
});
