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
  outputIssueHeadline,
  plainFailure,
  situationFailure,
} from '@prysm/application-client';
import { HeadlessWebPortal } from '../hosts/headless/web-portal/app';
import {
  lightCardView,
  moodOf,
  pauseOf,
} from '../packages/web-portal/light-card-view';
import { pathOf, sectionStatus } from '../packages/web-portal/section-state';
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
    ['unanswered', false],
    ['unanswered', true],
    ['faulted', false],
    ['unknown', false],
    ['unknown', true],
  ];
  const expected = {
    off: [
      'off',
      'off',
      'off',
      'off',
      'off',
      'unanswered',
      'unanswered',
      'faulted',
      'unknown',
      'unknown',
    ],
    screen_sync: [
      'starting',
      'failed',
      'sync',
      'starting',
      'failed',
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
      'unanswered',
      'unanswered',
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
  const input = { fps: 30, picture: 'arriving' };
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
  // A start of screen sync that failed from the lights' own effects: they
  // are still on, unless the lights themselves failed.
  const startFrom = (failed) =>
    lightingSituation({
      target: 'screen_sync',
      session: { state: 'effects', failure: failed },
    });
  for (const [failed, effectsOn] of [
    [failure, true],
    [{ kind: 'adapter', section: 'capture', message: 'x' }, true],
    [{ kind: 'adapter', section: 'output', message: 'x' }, false],
  ]) {
    const started = startFrom(failed);
    assert.equal(started.fromEffects, true, JSON.stringify(failed));
    assert.equal(started.effectsOn, effectsOn, JSON.stringify(failed));
    assert.equal(
      describeSituation(started, 'WLED effects').headline,
      'Screen sync couldn’t start',
      JSON.stringify(failed),
    );
  }
  const stoppedStart = lightingSituation({
    target: 'screen_sync',
    session: session('stopped', true),
  });
  assert.deepEqual(
    [stoppedStart.fromEffects, stoppedStart.effectsOn],
    [false, false],
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
  // Lights that don't answer Off or their effects: the target stays, and
  // the wait is said, with saved settings Prysm can't use beside it.
  // Screen sync asked meanwhile starts; its own session waits for them.
  for (const target of ['off', 'effects']) {
    assert.deepEqual(
      lightingSituation({ target, session: { state: 'unanswered' } }),
      { kind: 'unanswered', target, failure: null },
      target,
    );
    const beside = lightingSituation({
      target,
      session: { state: 'unanswered', failure: invalid },
    });
    assert.deepEqual(
      beside,
      { kind: 'unanswered', target, failure: invalid },
      target,
    );
    assert.equal(situationFailure(beside), invalid, target);
  }
  assert.equal(
    lightingSituation({
      target: 'screen_sync',
      session: { state: 'unanswered' },
    }).kind,
    'starting',
  );
  // The lights' own report is not the wait: the session says it.
  assert.equal(
    lightingSituation({
      target: 'off',
      session: session('stopped'),
      device: { issue: 'unreachable' },
    }).kind,
    'off',
  );
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
    headline: 'Screen sync couldn’t start',
    sentence: 'Screen sync couldn’t start.',
  });
  assert.deepEqual(words('effects', 'stopped', true), {
    headline: 'WLED effects couldn’t start',
    sentence: 'WLED effects couldn’t start.',
  });
  assert.deepEqual(words('screen_sync', 'faulted'), {
    headline: 'Prysm stopped responding',
    sentence: 'Prysm stopped responding. Restart Prysm to recover.',
  });
  assert.deepEqual(words('unknown', 'stopped'), {
    headline: 'Prysm is newer than this page',
    sentence:
      'Prysm is newer than this page. Reload it to see what the lights do.',
  });
  // Lights that don't answer: screen sync's headline, and what happens
  // once they answer.
  for (const [target, sentence] of [
    ['off', 'The lights switch off as soon as they answer.'],
    ['effects', 'WLED effects start as soon as the lights answer.'],
  ])
    assert.deepEqual(
      describeSituation(
        { kind: 'unanswered', target, failure: null },
        'WLED effects',
      ),
      {
        headline: 'Can’t reach the lights',
        sentence: `Can’t reach the lights. ${sentence}`,
      },
      target,
    );
  assert.equal(
    describeOutputIssue('sleeping'),
    'The lights aren’t showing screen sync.',
  );
  // Lights that don't answer are a problem; any other issue, a newer one
  // too, is not showing screen sync.
  assert.equal(outputIssueHeadline('unreachable'), 'Can’t reach the lights');
  for (const issue of ['off', 'other_sender', 'sleeping'])
    assert.equal(outputIssueHeadline(issue), 'Not showing screen sync', issue);
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
  const sync = { kind: 'sync', input: { fps: 30, picture: 'arriving' } };
  const failed = {
    kind: 'failed',
    target: 'screen_sync',
    failure,
    at: 'output',
    fromEffects: false,
    effectsOn: false,
  };
  assert.equal(moodOf(sync, true), 'sync');
  assert.equal(moodOf(failed, true), 'problem');
  assert.equal(moodOf({ kind: 'faulted', message }, true), 'problem');
  const newer = { kind: 'unknown', failure: null, message: null };
  assert.equal(moodOf(newer, true), 'unknown');
  const unknown = lightCardView(facts(newer, { target: 'unknown' }), memory());
  // No mode is chosen, and Power switches the lights off.
  assert.deepEqual(
    [unknown.word, unknown.on, unknown.chosen, unknown.status, unknown.info],
    [
      'Unknown',
      true,
      null,
      'Prysm is newer than this page',
      {
        tone: 'note',
        text: 'Prysm is newer than this page. Reload it to see what the lights do.',
        details: undefined,
      },
    ],
  );
  // What a newer state carries is said with the note, its words under Details.
  const carrying = lightCardView(
    facts(
      {
        kind: 'unknown',
        failure: { kind: 'adapter', section: 'output', message: 'gone' },
        message: 'newer',
      },
      { target: 'unknown' },
    ),
    memory(),
  );
  assert.deepEqual(carrying.info, {
    tone: 'note',
    text: 'Prysm is newer than this page. Reload it to see what the lights do. The lights aren’t responding.',
    details: 'gone newer',
  });
  assert.equal(
    plainFailure({ kind: 'adapter', message: 'x' }, 'unknown', 'WLED effects'),
    'Something went wrong.',
    'what a newer target tried is not known',
  );
  // A known target chooses its tile.
  assert.equal(
    lightCardView(facts(sync, { target: 'effects' }), memory()).chosen,
    'effects',
  );
  const faulted = lightCardView(facts({ kind: 'faulted', message }), memory());
  assert.deepEqual(
    [faulted.word, faulted.status, faulted.info],
    [
      'Stopped',
      'Prysm stopped responding',
      {
        tone: 'problem',
        text: 'Prysm stopped responding. Restart Prysm to recover.',
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
  // Another kind is said in its own words.
  const effectsAdapter = lightCardView(
    facts(
      {
        kind: 'effects',
        failure: { kind: 'adapter', section: 'output', message: 'gone' },
      },
      { target: 'effects' },
    ),
    memory({ lit: true }),
  );
  assert.deepEqual(effectsAdapter.info, {
    tone: 'problem',
    text: 'The lights aren’t responding.',
    details: 'gone',
  });
  // A start of screen sync that failed from the effects: they are still on.
  const fromEffects = lightCardView(
    facts({ ...failed, at: null, fromEffects: true, effectsOn: true }),
    memory(),
  );
  assert.equal(
    fromEffects.info.text,
    'Screen sync couldn’t start. WLED effects are still on.',
  );
  assert.equal(fromEffects.info.retry, true);
  // The lights themselves failed: not said to be still on.
  const lightsFailed = lightCardView(
    facts({
      ...failed,
      failure: { kind: 'adapter', section: 'output', message: 'gone' },
      fromEffects: true,
      effectsOn: false,
    }),
    memory(),
  );
  assert.equal(lightsFailed.info.text, 'The lights aren’t responding.');
  assert.equal(lightsFailed.status, 'Screen sync couldn’t start');
  // Lights settings Prysm can't use never reached the lights: the effects
  // are still on, and trying again could not help.
  const unusable = lightingSituation({
    target: 'screen_sync',
    session: { state: 'effects', failure: invalid },
  });
  assert.equal(unusable.kind === 'failed' && unusable.effectsOn, true);
  const unusableCard = lightCardView(facts(unusable), memory());
  assert.deepEqual(
    [unusableCard.info.text, unusableCard.info.retry],
    ['Prysm can’t use these settings. WLED effects are still on.', false],
  );
  assert.equal(
    moodOf({ kind: 'starting', target: 'screen_sync' }, true),
    'starting',
  );
  assert.equal(moodOf(sync, false), 'unset');

  assert.equal(
    pauseOf({ kind: 'sync', input: { fps: 0, picture: 'quiet' } }, true),
    'quiet',
  );
  assert.equal(
    pauseOf(
      {
        kind: 'sync',
        input: { fps: 0, picture: 'refused' },
      },
      true,
    ),
    'refused',
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
    text: 'Screen sync couldn’t start.',
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
  // Lights that don't answer Off or their effects: neither lit nor busy,
  // the mode kept, and a warning as during screen sync.
  for (const [target, mode, text] of [
    [
      'off',
      'screen_sync',
      'Can’t reach the lights. The lights switch off as soon as they answer.',
    ],
    [
      'effects',
      'effects',
      'Can’t reach the lights. WLED effects start as soon as the lights answer.',
    ],
  ]) {
    const unanswered = { kind: 'unanswered', target, failure: null };
    assert.equal(moodOf(unanswered, true), 'off', target);
    assert.equal(pauseOf(unanswered, false), null, target);
    const view = lightCardView(
      facts(unanswered, { target, device: { issue: 'unreachable' } }),
      memory(),
    );
    assert.deepEqual(
      [view.word, view.on, view.chosen, view.status, view.info, view.calm],
      [
        'Off',
        target !== 'off',
        mode,
        'Can’t reach the lights',
        { tone: 'warning', text },
        '',
      ],
      target,
    );
    assert.equal(view.ownEffects, false, target);
  }
  // Saved settings Prysm can't use, beside the wait: the wait is the
  // status, and the settings are the line.
  const beside = lightCardView(
    facts(
      { kind: 'unanswered', target: 'off', failure: invalid },
      { target: 'off', device: { issue: 'unreachable' } },
    ),
    memory(),
  );
  assert.deepEqual(
    [beside.status, beside.info],
    [
      'Can’t reach the lights',
      {
        tone: 'problem',
        text: 'Prysm can’t use these settings.',
        details: invalid.message,
        retry: false,
      },
    ],
  );
});

const capture = { kind: 'device', device: '/dev/video0' };
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
      message: 'Prysm didn’t answer.',
    });
    await press($('[data-power]'));
    assert(shown());
    assert.equal(
      $('.light-info > div > p').textContent,
      'A device isn’t available right now.',
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
    'The lights aren’t responding.',
  );
  assert.equal(
    plainFailure({ kind: 'adapter', message: 'x' }, 'off', 'WLED effects'),
    'The lights couldn’t be switched off.',
  );
  assert.equal(
    plainFailure({ kind: 'invalid', message: 'x' }, 'off', 'WLED effects'),
    'Prysm can’t use these settings.',
  );
  // Nothing to try again with the lights off.
  assert.equal(
    plainFailure({ kind: 'busy', message: 'x' }, 'off', 'WLED effects'),
    'Prysm is busy.',
  );
  // As the engine reports saved settings that do not validate: the field
  // names its section, yet a setting is the fix.
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
    text: 'The lights aren’t responding.',
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
  assert.equal(refused.info.text, 'Screen sync couldn’t start.');
});

test('with the lights off the path marks a failure only where it is', () => {
  // A failure, then the path; a failed switch off without a side is said by
  // the light card alone, never as a broken Prysm.
  for (const [failure, path] of [
    [
      { kind: 'invalid', field: 'capture.width', message: 'x' },
      { kind: 'failed', at: 'capture' },
    ],
    [invalid, { kind: 'failed', at: 'output' }],
    [
      { kind: 'invalid', field: 'processing.gamma', message: 'x' },
      { kind: 'failed', at: null },
    ],
    [
      { kind: 'adapter', section: 'capture', message: 'x' },
      { kind: 'failed', at: 'capture' },
    ],
    [
      { kind: 'adapter', section: 'output', message: 'x' },
      { kind: 'failed', at: 'output' },
    ],
    [{ kind: 'adapter', message: 'x' }, { kind: 'off' }],
    [{ kind: 'unavailable', message: 'x' }, { kind: 'off' }],
    [{ kind: 'busy', message: 'x' }, { kind: 'off' }],
    [{ kind: 'newer', message: 'x' }, { kind: 'off' }],
  ])
    assert.deepEqual(
      pathOf(
        lightingSituation({
          target: 'off',
          session: { state: 'stopped', failure },
        }),
      ),
      path,
      JSON.stringify(failure),
    );
  assert.deepEqual(
    pathOf(lightingSituation({ target: 'off', session: { state: 'stopped' } })),
    { kind: 'off' },
  );
  // Screen sync without a picture waits, and says why.
  for (const [input, picture] of [
    [undefined, 'quiet'],
    [{ fps: 0, picture: 'quiet' }, 'quiet'],
    [{ fps: 0, picture: 'refused' }, 'refused'],
    [{ fps: 0, picture: 'newer' }, 'quiet'],
  ])
    assert.deepEqual(
      pathOf(
        lightingSituation({
          target: 'screen_sync',
          session: { state: 'screen_sync' },
          input,
        }),
      ),
      { kind: 'waiting', picture },
      JSON.stringify(input),
    );
  // A refused video is never a problem, however long it lasts.
  const refused = lightingSituation({
    target: 'screen_sync',
    session: { state: 'screen_sync' },
    input: { fps: 0, picture: 'refused' },
  });
  for (const section of ['capture', 'output'])
    assert.deepEqual(
      sectionStatus(
        section,
        {
          online: true,
          target: 'screen_sync',
          path: pathOf(refused),
          failure: situationFailure(refused),
          situation: refused,
          device: {},
        },
        'WLED at 192.0.2.1',
        true,
      ),
      {
        tone: 'idle',
        title: 'No picture',
        detail: 'The TV doesn’t let Prysm capture this video.',
      },
      section,
    );
});

test('with the lights off a section page says the failure it names, else not in use', () => {
  const identity = 'WLED at 192.0.2.1';
  const idle = 'Not in use';
  // A failure, then the video source's and the lights' status.
  for (const [failure, video, lights] of [
    [
      { kind: 'invalid', field: 'capture.width', message: 'x' },
      ['problem', 'Can’t use these settings', '', 'x'],
      idle,
    ],
    [
      invalid,
      idle,
      ['problem', 'Can’t use these settings', identity, invalid.message],
    ],
    [
      { kind: 'adapter', section: 'capture', message: 'x' },
      ['problem', 'Video source not working', '', 'x'],
      idle,
    ],
    [
      { kind: 'adapter', section: 'output', message: 'x' },
      idle,
      ['problem', 'Lights not responding', identity, 'x'],
    ],
    // Without a side: neither section's own, as the setup path shows it.
    [{ kind: 'adapter', message: 'x' }, idle, idle],
    [{ kind: 'invalid', field: 'processing.gamma', message: 'x' }, idle, idle],
  ]) {
    const situation = lightingSituation({
      target: 'off',
      session: { state: 'stopped', failure },
    });
    const live = {
      online: true,
      target: 'off',
      path: pathOf(situation),
      failure: situationFailure(situation),
      situation,
      device: {},
    };
    for (const [section, expected] of [
      ['capture', video],
      ['output', lights],
    ]) {
      const status = sectionStatus(section, live, identity, false);
      if (typeof expected === 'string')
        assert.deepEqual(
          [status.tone, status.title],
          ['idle', expected],
          JSON.stringify([failure, section]),
        );
      else
        assert.deepEqual(
          [status.tone, status.title, status.detail, status.details],
          expected,
          JSON.stringify([failure, section]),
        );
    }
  }
});

test('settings Prysm can’t use are said as such, whatever the target', () => {
  // As the engine reports settings that do not validate at a start of screen
  // sync, or a video source that can't deliver them: the field names a
  // section, yet a setting is the fix.
  assert.equal(
    plainFailure(
      { kind: 'invalid', field: 'output.address', message: 'x' },
      'screen_sync',
      'WLED effects',
    ),
    'Prysm can’t use these settings.',
  );
  for (const target of ['off', 'screen_sync', 'effects', 'unknown'])
    for (const failure of [
      invalid,
      { kind: 'invalid', field: 'capture.width', message: 'x' },
      { kind: 'invalid', message: 'x' },
    ])
      assert.equal(
        plainFailure(failure, target, 'WLED effects'),
        'Prysm can’t use these settings.',
        JSON.stringify([failure, target]),
      );
  // The section it names says so; the other one says screen sync stopped.
  const identity = 'WLED at 192.0.2.1';
  const unusable = 'Can’t use these settings';
  const stopped = [
    'idle',
    'Screen sync stopped',
    'Prysm can’t use these settings.',
    undefined,
  ];
  for (const [failure, video, lights] of [
    [invalid, stopped, ['problem', unusable, identity, invalid.message]],
    [
      { kind: 'invalid', field: 'capture.width', message: 'x' },
      ['problem', unusable, '', 'x'],
      stopped,
    ],
    // Without a side: both sections' settings.
    [
      { kind: 'invalid', field: 'processing.gamma', message: 'x' },
      ['problem', unusable, '', 'x'],
      ['problem', unusable, identity, 'x'],
    ],
  ]) {
    const situation = lightingSituation({
      target: 'screen_sync',
      session: { state: 'stopped', failure },
    });
    const live = {
      online: true,
      target: 'screen_sync',
      path: pathOf(situation),
      failure: situationFailure(situation),
      situation,
      device: {},
    };
    for (const [section, expected] of [
      ['capture', video],
      ['output', lights],
    ]) {
      const status = sectionStatus(section, live, identity, false);
      assert.deepEqual(
        [status.tone, status.title, status.detail, status.details],
        expected,
        JSON.stringify([failure, section]),
      );
    }
  }
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
      'The lights aren’t responding.',
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

test('lights that don’t answer Off or their effects are said on the lights page as during screen sync', () => {
  const identity = 'WLED at 192.0.2.1';
  const device = { issue: 'unreachable' };
  const unreachableStatus = {
    tone: 'problem',
    title: 'Can’t reach the lights',
    detail: `${identity}. The lights don’t answer. Check that they are powered and on the network.`,
  };
  const liveOf = (target, session) => {
    const situation = lightingSituation({ target, session });
    return {
      online: true,
      target,
      path: pathOf(situation),
      failure: situationFailure(situation),
      situation,
      device,
    };
  };
  const unusable = {
    kind: 'invalid',
    field: 'capture.device',
    message: 'a video device is required',
  };
  for (const target of ['off', 'effects']) {
    const live = liveOf(target, { state: 'unanswered' });
    assert.deepEqual(live.path, { kind: 'off' }, target);
    assert.deepEqual(
      sectionStatus('output', live, identity, false),
      unreachableStatus,
      target,
    );
    assert.equal(
      sectionStatus('capture', live, identity, false).title,
      'Not in use',
      target,
    );
    // Saved settings Prysm can't use, beside the wait, on their own side.
    const beside = liveOf(target, { state: 'unanswered', failure: unusable });
    assert.deepEqual(beside.path, { kind: 'failed', at: 'capture' }, target);
    assert.deepEqual(
      sectionStatus('output', beside, identity, false),
      unreachableStatus,
      target,
    );
    assert.deepEqual(
      sectionStatus('capture', beside, identity, false),
      {
        tone: 'problem',
        title: 'Can’t use these settings',
        detail: '',
        details: unusable.message,
      },
      target,
    );
  }
});

test('the light card says lights that don’t answer, and what Prysm does once they do', async () => {
  const portal = await renderPortal(HeadlessWebPortal, { capture, output });
  const { fixture } = portal;
  try {
    for (const [target, text] of [
      ['off', 'The lights switch off as soon as they answer.'],
      ['effects', 'WLED effects start as soon as the lights answer.'],
    ]) {
      await act(async () =>
        fixture.publish({
          target,
          session: { state: 'unanswered' },
          device: { issue: 'unreachable' },
        }),
      );
      assert(shown(), target);
      assert.equal(
        $('.light-info > div > p').textContent,
        `Can’t reach the lights. ${text}`,
        target,
      );
      assert.equal($('.light-info').dataset.tone, 'warning', target);
      assert.equal(
        $('.lighting-status').textContent,
        'Can’t reach the lights',
        target,
      );
      assert.equal(
        $('.light-word [data-shown="true"]').textContent,
        'Off',
        target,
      );
    }
    // Once they answer, the chosen mode says itself again.
    await act(async () =>
      fixture.publish({ session: { state: 'effects' }, device: {} }),
    );
    assert.equal($('.lighting-status').textContent, 'WLED effects are on');
  } finally {
    await portal.unmount();
  }
});
