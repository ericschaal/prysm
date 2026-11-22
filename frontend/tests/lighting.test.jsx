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
} from '@prysm/application-client';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import {
  lightCardView,
  moodOf,
  pauseOf,
} from '../packages/web-portal/light-card-view';
import { $, navigated, renderPortal } from './dom';

const failure = { kind: 'adapter', message: 'WLED cannot be reached' };
const session = (state) =>
  state === 'faulted' ? { state, phase: 'finishing' } : { state };

test('every target, session and failure has one lighting situation', () => {
  // target, session, failure, situation
  const rows = [
    ['off', 'stopped', false, 'off'],
    ['off', 'running', false, 'off'],
    ['off', 'native', false, 'off'],
    ['off', 'faulted', false, 'faulted'],
    ['off', 'stopped', true, 'off'],
    ['off', 'running', true, 'off'],
    ['off', 'native', true, 'off'],
    ['off', 'faulted', true, 'faulted'],
    ['processed', 'stopped', false, 'starting'],
    ['processed', 'running', false, 'sync'],
    ['processed', 'native', false, 'starting'],
    ['processed', 'faulted', false, 'faulted'],
    ['processed', 'stopped', true, 'failed'],
    ['processed', 'running', true, 'failed'],
    ['processed', 'native', true, 'failed'],
    ['processed', 'faulted', true, 'faulted'],
    ['device_native', 'stopped', false, 'starting'],
    ['device_native', 'running', false, 'starting'],
    ['device_native', 'native', false, 'effects'],
    ['device_native', 'faulted', false, 'faulted'],
    ['device_native', 'stopped', true, 'failed'],
    ['device_native', 'running', true, 'failed'],
    ['device_native', 'native', true, 'failed'],
    ['device_native', 'faulted', true, 'faulted'],
  ];
  for (const [target, state, failed, kind] of rows) {
    const situation = lightingSituation({
      target,
      session: session(state),
      ...(failed ? { failure } : {}),
    });
    assert.equal(situation.kind, kind, `${target} ${state} ${failed}`);
  }
  const input = { fps: 30, receiving: true };
  assert.deepEqual(
    lightingSituation({
      target: 'processed',
      session: session('running'),
      input,
    }),
    { kind: 'sync', input },
  );
  assert.deepEqual(
    lightingSituation({ target: 'processed', session: session('running') }),
    { kind: 'sync', input: undefined },
    'not measured yet is not "not receiving"',
  );
  assert.deepEqual(
    lightingSituation({ target: 'device_native', session: session('stopped') }),
    { kind: 'starting', target: 'device_native' },
  );
  assert.deepEqual(
    lightingSituation({ target: 'processed', session: session('faulted') }),
    { kind: 'faulted', phase: 'finishing' },
  );
  // The side that failed: typed, else the setting named.
  const at = (change) =>
    lightingSituation({
      target: 'processed',
      session: session('stopped'),
      failure: { ...failure, ...change },
    }).at;
  assert.equal(at({}), null);
  assert.equal(at({ section: 'capture' }), 'capture');
  assert.equal(at({ kind: 'invalid', field: 'output.leds' }), 'output');
  assert.equal(at({ kind: 'invalid', field: 'processing.style' }), null);
});

test('each situation has one headline and one sentence', () => {
  const words = (target, state, failed = false) =>
    describeSituation(
      lightingSituation({
        target,
        session: session(state),
        ...(failed ? { failure } : {}),
      }),
      'WLED effects',
    );
  assert.deepEqual(words('off', 'running'), {
    headline: 'Lights are off',
    sentence: 'Lights are off.',
  });
  assert.deepEqual(words('processed', 'running'), {
    headline: 'Lights follow your screen',
    sentence: 'Screen sync is running.',
  });
  assert.deepEqual(words('device_native', 'native'), {
    headline: 'WLED effects are on',
    sentence: 'WLED effects are on.',
  });
  assert.deepEqual(words('processed', 'native'), {
    headline: 'Starting screen sync…',
    sentence: 'Starting screen sync…',
  });
  assert.deepEqual(words('device_native', 'running'), {
    headline: 'Starting WLED effects…',
    sentence: 'Starting WLED effects…',
  });
  assert.deepEqual(words('processed', 'native', true), {
    headline: 'Screen sync stopped',
    sentence: 'Screen sync stopped.',
  });
  assert.deepEqual(words('device_native', 'stopped', true), {
    headline: 'WLED effects could not start',
    sentence: 'WLED effects could not start.',
  });
  const faulted =
    'Prysm stopped responding while finishing. Restart Prysm to recover.';
  assert.deepEqual(words('processed', 'faulted'), {
    headline: faulted,
    sentence: faulted,
  });
  // Values newer than this page still have words.
  assert.equal(
    describeSituation(
      { kind: 'faulted', phase: 'cooling_down' },
      'WLED effects',
    ).headline,
    'Prysm stopped responding. Restart Prysm to recover.',
  );
  assert.equal(
    describeOutputIssue('sleeping'),
    'The lights are not showing screen sync.',
  );
});

const facts = (lighting, change = {}) => ({
  target: 'processed',
  lighting,
  device: {},
  hasCapture: true,
  hasOutput: true,
  output: { effects: 'WLED effects' },
  ...change,
});
const memory = (change = {}) => ({
  remembered: 'processed',
  error: null,
  lit: false,
  note: null,
  ...change,
});

test('the light card view: mood, word, line and mode from the situation', () => {
  const sync = { kind: 'sync', input: { fps: 30, receiving: true } };
  const failed = {
    kind: 'failed',
    target: 'processed',
    failure,
    at: 'output',
  };
  assert.equal(moodOf(sync, true), 'sync');
  assert.equal(moodOf(failed, true), 'problem');
  assert.equal(moodOf({ kind: 'faulted', phase: 'running' }, true), 'problem');
  assert.equal(
    moodOf({ kind: 'starting', target: 'processed' }, true),
    'starting',
  );
  assert.equal(moodOf(sync, false), 'unset');

  assert.equal(
    pauseOf({ kind: 'sync', input: { fps: 0, receiving: false } }, true),
    'waiting',
  );
  assert.equal(pauseOf({ kind: 'sync', input: undefined }, true), null);
  assert.equal(
    pauseOf({ kind: 'starting', target: 'processed' }, false),
    'starting',
  );
  assert.equal(pauseOf({ kind: 'starting', target: 'processed' }, true), null);

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
    facts({ kind: 'off' }, { target: 'off' }),
    memory({ error: { kind: 'busy', message: 'busy' } }),
  );
  assert.equal(refused.info.text, 'Prysm is busy. Try again in a moment.');
  assert.equal(refused.info.retry, false);
  // Off: the chosen mode is the remembered one, if it can run.
  const off = lightCardView(
    facts({ kind: 'off' }, { target: 'off' }),
    memory({ remembered: 'device_native' }),
  );
  assert.deepEqual(
    [off.mode, off.calm],
    ['device_native', 'Power turns on WLED effects.'],
  );
  assert.equal(off.ownEffects, true);
  const noEffects = lightCardView(
    facts({ kind: 'off' }, { target: 'off', output: {} }),
    memory({ remembered: 'device_native' }),
  );
  assert.equal(noEffects.mode, 'processed');
  assert.equal(noEffects.calm, 'Power turns on Screen sync.');
  // An output issue is said only while screen sync runs.
  const issue = { device: { issue: 'other_sender' } };
  assert.equal(
    lightCardView(facts(sync, issue), memory({ lit: true })).info.tone,
    'warning',
  );
  assert.equal(
    lightCardView(facts({ kind: 'off' }, { ...issue, target: 'off' }), memory())
      .info,
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
      fixture.publish({ target: 'processed', session: { state: 'running' } }),
    );
    assert.equal(shown(), false);
    // A refused click while it runs stays while nothing changes.
    fixture.failNext({ kind: 'busy', message: 'busy' });
    await press($('[data-target="device_native"]'));
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
