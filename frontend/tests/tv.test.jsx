/**
 * The TV app on the real Luna client over an in-memory supervisor, and the
 * portal over the application fixture. Ported from the earlier TV app's
 * behaviour tests (crates/web/prysm-webos/tests/app.test.cjs in the tag
 * `pre-1.0-consolidation`) to the current state.
 */
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { JSDOM } from 'jsdom';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { ApplicationError, lightingSituation } from '@prysm/application-client';
import {
  APP_ID,
  CALL_TIMEOUT_MS,
  HOMEBREW_EXEC,
  RESUBSCRIBE_MS,
  SERVICE_URI,
  SETUP_SCRIPT,
  decodeState,
  lunaTvService,
} from '@prysm/webos-platform';
import { TvApp } from '@prysm/tv-ui';
import { diagnostics } from '../packages/tv-ui/diagnostics';
import {
  completePreferences,
  localPortal,
  prysmAddress,
  sentPreferences,
  shown,
} from '../packages/tv-ui/preferences';
import { captureRate, encodingName, tvScreen } from '../packages/tv-ui/screen';
import { tvStatus } from '../packages/tv-ui/status';
import { initialService, serviceReducer } from '../packages/tv-ui/service';
import { fakeApplicationPort } from '../testkit/application';
import { fakeTvSupervisor } from '../testkit/luna';
import {
  PATTERN_TYPE,
  STALL_MS,
  msePatternPlayer,
  streamPatterns,
} from '../packages/tv-ui/patterns';
import { qrCode } from '../packages/tv-ui/qr';
import { STALE_MS } from '../packages/tv-ui/portal-connection';
import { createHash } from 'node:crypto';
import jsQR from 'jsqr';
import { assertFocused, assertSame } from './dom';

const fixtures = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/contracts/generated/fixtures.json'),
    'utf8',
  ),
);
const tick = () => new Promise((done) => setImmediate(done));
/** The service subscription as `tvStatus` reads it. */
const live = (state) => ({ status: 'live', state });
const connecting = { status: 'connecting' };
const missing = { status: 'missing', state: null };
async function settle(rounds = 8) {
  await act(async () => {
    for (let round = 0; round < rounds; round++) await tick();
  });
}

const wled = {
  kind: 'wled',
  address: '192.168.1.50',
  leds: { top: 96, right: 54, bottom: 96, left: 54 },
  extra_field: 'kept',
};
const portals = {
  full: {
    capture: { kind: 'webos', width: 640, height: 360, fps: 30 },
    output: wled,
  },
  slim: { capture: { kind: 'remote' }, output: wled },
};

async function mountTv(t, options = {}) {
  const flavor = options.flavor ?? 'slim';
  const dom = new JSDOM('<div id="root"></div>', { pretendToBeVisual: true });
  Object.assign(globalThis, {
    window: dom.window,
    document: dom.window.document,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const supervisor = fakeTvSupervisor({ flavor, ...options.supervisor });
  // No reply yet: the screen before the service answers.
  if (options.hold) supervisor.hold();
  const application = fakeApplicationPort(options.portal ?? portals[flavor]);
  // The full worker reports its engine's failure to the supervisor.
  if (flavor === 'full')
    t.after(application.watch((snapshot) => supervisor.report(snapshot)));
  const urls = [];
  const portal = {
    client: (url) => {
      urls.push(url);
      return options.block
        ? {
            ...application.client,
            observe: () =>
              Promise.reject(
                new ApplicationError(
                  options.block === true
                    ? {
                        kind: 'unavailable',
                        message: 'Can’t reach Prysm.',
                      }
                    : options.block,
                ),
              ),
          }
        : application.client;
    },
    probe: options.probe ?? (() => Promise.resolve(false)),
    supported: options.supported ?? true,
  };
  let exits = 0;
  const root = createRoot(document.getElementById('root'));
  await act(async () =>
    root.render(
      <TvApp
        flavor={flavor}
        service={lunaTvService(supervisor.bridge)}
        portal={portal}
        onExit={() => exits++}
        patterns={options.patterns}
      />,
    ),
  );
  await settle();
  let mounted = true;
  async function unmount() {
    if (!mounted) return;
    mounted = false;
    await act(async () => root.unmount());
  }
  t.after(async () => {
    await unmount();
    dom.window.close();
  });
  const el = (id) => document.getElementById(id);
  return {
    dom,
    supervisor,
    application,
    urls,
    el,
    unmount,
    exits: () => exits,
    focus: () => document.activeElement?.id,
    text: (id) => el(id)?.textContent ?? '',
    methods: () => supervisor.calls.map((call) => call.method),
    click: async (id) => {
      await act(async () => el(id).click());
      await settle();
    },
    key: async (code, extra = {}) => {
      await act(async () =>
        (document.activeElement ?? document.body).dispatchEvent(
          new dom.window.KeyboardEvent('keydown', {
            keyCode: code,
            bubbles: true,
            cancelable: true,
            ...extra,
          }),
        ),
      );
      await settle();
    },
    choose: async (label) => {
      const choice = [...document.querySelectorAll('#dialog-body button')].find(
        (item) => item.textContent.startsWith(label),
      );
      assert(choice, `no choice ${label}`);
      await act(async () => choice.click());
      await settle();
    },
    type: async (id, value) => {
      const input = el(id);
      await act(async () => {
        Object.getOwnPropertyDescriptor(
          dom.window.HTMLInputElement.prototype,
          'value',
        ).set.call(input, value);
        input.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
      });
    },
    submit: async (id) => {
      await act(async () =>
        el(id)
          .closest('form')
          .dispatchEvent(
            new dom.window.Event('submit', { bubbles: true, cancelable: true }),
          ),
      );
      await settle();
    },
  };
}
const [LEFT, UP, RIGHT, DOWN, BACK, TAB, ESCAPE] = [37, 38, 39, 40, 461, 9, 27];

/* ---- Luna client ---- */

function manualBridge() {
  const channels = [];
  const factory = () => {
    const channel = {
      onservicecallback: null,
      cancelled: 0,
      call(uri, payload) {
        channel.uri = uri;
        channel.payload = JSON.parse(payload);
      },
      cancel() {
        channel.cancelled++;
        // Some bridges call back while cancelling; that must be ignored.
        channel.onservicecallback?.('{"returnValue":false}');
      },
      reply: (value) =>
        channel.onservicecallback(
          typeof value === 'string' ? value : JSON.stringify(value),
        ),
    };
    channels.push(channel);
    return channel;
  };
  return { factory, channels };
}
/** The full package's state as Rust serializes it (lighting on, capturing). */
const state = (change = {}) => ({ ...fixtures['luna/State'][0], ...change });
/**
 * A state of `flavor` as the supervisor publishes it, from its facts (see
 * testkit/luna.ts): only that package's keys are accepted.
 */
const tvState = (flavor, facts = {}, preferences) =>
  fakeTvSupervisor({ flavor, preferences, state: facts }).state();
/** What the service asks: `ask` with `fix`, or the lights that stopped answering. */
const ask = (fix) => ({ step: 'ask', fix });
const UNANSWERED = { step: 'unanswered' };
/** Every kind of what is asked. */
const everyAsked = [
  UNANSWERED,
  ...[
    'try_again',
    'change_settings',
    'set_up',
    'unsupported',
    'restart_tv',
  ].map(ask),
];
/** Facts of a supervisor that asks `asked` with no worker, for `message`. */
const asking = (asked, message = 'x') => ({
  worker: { phase: 'asking', problem: { asked, message } },
});

test('generated Rust states decode as they are; malformed ones do not', () => {
  for (const value of fixtures['luna/State'])
    assert.deepEqual(decodeState(value), value, JSON.stringify(value));
  const malformed = (value) => decodeState(value) === null;
  assert(malformed({ ...state(), flavor: 'lite' }), 'a package has its fields');
  assert(malformed({ ...state(), worker: 'idle' }));
  assert(
    malformed({ ...state(), worker: 'running' }),
    'the phase is an object',
  );
  assert(
    malformed({ ...state(), worker: { phase: 'asking' } }),
    'asking says what it asks',
  );
  assert(
    malformed({
      ...state(),
      worker: { ...state().worker, problem: { asked: 'try_again' } },
    }),
    'a problem says what is asked, and why',
  );
  assert(
    malformed({
      ...state(),
      worker: {
        phase: 'asking',
        problem: { asked: { step: 'ask' }, message: 'x' },
      },
    }),
    '`ask` says its fix',
  );
  assert(malformed({ ...state(), unreadable: 3 }), 'unreadable says why');
  assert(malformed({ ...state(), unreadable: null }), 'absent, not null');
  assert(malformed({ ...state(), power: 3 }));
  assert(malformed({ ...state(), worker: { ...state().worker, fps: '30' } }));
  assert(malformed({ ...state(), worker: { ...state().worker, lights: 3 } }));
  assert(malformed({ ...state(), log: [1] }));
  assert(
    malformed({ ...state(), preferences: [] }),
    'preferences are an object',
  );
  assert(malformed(null));
  // Each package has its own fields: full its target, slim `enabled` and
  // a worker that reports its stream.
  assert(malformed({ ...state(), target: null }), 'full has a target');
  assert(malformed({ ...state(), flavor: 'slim' }), 'slim says if enabled');
  const slim = fixtures['luna/State'].find((value) => value.flavor === 'slim');
  assert(malformed({ ...slim, flavor: 'full' }), 'full has no `enabled`');
  assert(
    malformed({
      ...slim,
      worker: { phase: 'running', capturing: true, fps: 30 },
    }),
    'a running slim worker says where its link stands',
  );
  assert(
    malformed({
      ...slim,
      worker: {
        phase: 'running',
        capturing: false,
        link: { state: 'waiting' },
      },
    }),
    'a waiting link says why',
  );
  assert(
    malformed({ ...slim, worker: { phase: 'stopping', problem: 'x' } }),
    'a stop that asks says what',
  );
  assert(
    decodeState(tvState('slim', { worker: { phase: 'running' } })) &&
      decodeState(tvState('full', { worker: { phase: 'stopping' } })),
    'the fake supervisor publishes valid states',
  );
});

test('values newer than the app have one reading each', () => {
  // A newer service's state was refused whole: the app offered to set up a
  // service that was running.
  assert.deepEqual(decodeState({ ...state(), power: 'dimmed', video: 'hdr' }), {
    ...state(),
    power: 'unknown',
    video: 'unknown',
  });
  // What is asked: a newer step or fix asks to try again.
  const askingState = (asked) => ({ ...state(), ...asking(asked) });
  for (const asked of [{ step: 'wait' }, ask('reboot')])
    assert.deepEqual(
      decodeState(askingState(asked)),
      askingState(ask('try_again')),
    );
  // Never read as off or stopped: a target or phase newer than the app.
  assert.deepEqual(decodeState({ ...state(), target: 'ambient' }), {
    ...state(),
    target: 'unknown',
  });
  assert.deepEqual(decodeState({ ...state(), worker: { phase: 'paused' } }), {
    ...state(),
    worker: { phase: 'unknown' },
  });
  // A newer lights issue is kept; the status card says it in general words
  // and points to the lights card.
  const dimmed = {
    ...state(),
    worker: { ...state().worker, lights: 'dimmed' },
  };
  assert.deepEqual(decodeState(dimmed), dimmed);
  const newer = tvStatus(live(decodeState(dimmed)), completePreferences({}));
  assert.deepEqual(
    [newer.title, newer.description],
    ['Not showing screen sync', 'See Your lights for what to do.'],
  );
  const slim = fixtures['luna/State'].find((value) => value.flavor === 'slim');
  const link = (value) =>
    decodeState({
      ...slim,
      worker: { phase: 'running', capturing: true, link: value },
    })?.worker.link;
  assert.deepEqual(link({ state: 'reconnecting' }), { state: 'starting' });
  assert.deepEqual(link({ state: 'refused' }), { state: 'refused' });
  // A newer encoding is shown as the service names it.
  assert.deepEqual(link({ state: 'sending', encoding: 'h265', fps: 30 }), {
    state: 'sending',
    encoding: 'h265',
    fps: 30,
  });
  assert.equal(encodingName('h265'), 'h265');
  assert.deepEqual(link({ state: 'stalled', message: 'x' }), {
    state: 'starting',
  });
});

test('the state subscription retries after unsubscribed, malformed and refused replies', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const bridge = manualBridge();
  const events = [];
  const stop = lunaTvService(bridge.factory).watch((event) =>
    events.push(event),
  );
  assert.equal(bridge.channels[0].uri, `${SERVICE_URI}/getState`);
  assert.deepEqual(bridge.channels[0].payload, { subscribe: true });
  bridge.channels[0].reply({
    returnValue: true,
    subscribed: false,
    ...state(),
  });
  assert.equal(events.at(-1).online, false);
  assert.equal(bridge.channels[0].cancelled, 1);
  assert.equal(events.length, 1, 'the callback during cancel is ignored');
  t.mock.timers.tick(RESUBSCRIBE_MS);
  bridge.channels[1].reply('not json');
  assert.equal(events.at(-1).error.kind, 'invalid_reply');
  t.mock.timers.tick(RESUBSCRIBE_MS);
  bridge.channels[2].reply({
    returnValue: true,
    subscribed: true,
    ...state({ worker: 'idle' }),
  });
  assert.equal(events.at(-1).error.kind, 'invalid_reply');
  t.mock.timers.tick(RESUBSCRIBE_MS);
  bridge.channels[3].reply({
    returnValue: true,
    subscribed: true,
    ...state({ target: 'off' }),
  });
  bridge.channels[3].reply({
    returnValue: true,
    ...state({ target: 'screen_sync' }),
  });
  assert.deepEqual(
    events.slice(-2).map((event) => event.online && event.state.target),
    ['off', 'screen_sync'],
  );
  assert.equal('returnValue' in events.at(-1).state, false);
  bridge.channels[3].reply({
    returnValue: false,
    errorText: 'Service stopped',
  });
  assert.equal(events.at(-1).error.kind, 'refused');
  stop();
  t.mock.timers.tick(RESUBSCRIBE_MS);
  assert.equal(
    bridge.channels.length,
    4,
    'a stopped watch does not resubscribe',
  );
});

test('a silent service times out after 30 s; late replies are ignored', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const bridge = manualBridge();
  const service = lunaTvService(bridge.factory);
  const events = [];
  const stop = service.watch((event) => events.push(event));
  t.mock.timers.tick(CALL_TIMEOUT_MS - 1);
  assert.equal(events.length, 0);
  t.mock.timers.tick(1);
  assert.equal(events[0].error.kind, 'timeout');
  bridge.channels[0].onservicecallback?.(
    JSON.stringify({ returnValue: true, subscribed: true, ...state() }),
  );
  assert.equal(events.length, 1);
  stop();

  const targeting = service.setTarget('screen_sync');
  t.mock.timers.tick(CALL_TIMEOUT_MS);
  await assert.rejects(targeting, { kind: 'timeout' });
  const channel = bridge.channels.at(-1);
  assert.equal(channel.cancelled, 1);
  // A retry works after the timeout.
  const again = service.setTarget('screen_sync');
  assert.equal(bridge.channels.at(-1).uri, `${SERVICE_URI}/setTarget`);
  assert.deepEqual(bridge.channels.at(-1).payload, { target: 'screen_sync' });
  bridge.channels
    .at(-1)
    .reply({ returnValue: true, ...state({ target: 'screen_sync' }) });
  assert.equal((await again).target, 'screen_sync');
});

test('commands carry the contract payloads and fail with typed errors', async () => {
  const bridge = manualBridge();
  const service = lunaTvService(bridge.factory);
  const saving = service.setPreferences({ autostart: true });
  assert.equal(bridge.channels[0].uri, `${SERVICE_URI}/setPreferences`);
  assert.deepEqual(bridge.channels[0].payload, { autostart: true });
  bridge.channels[0].reply({
    returnValue: false,
    errorKind: 'forbidden',
    errorText: `only ${APP_ID} may change Prysm`,
  });
  await assert.rejects(saving, {
    kind: 'refused',
    errorKind: 'forbidden',
    message: `only ${APP_ID} may change Prysm`,
  });
  // A kind newer than this app, and LS2's own reply without one: `unknown`,
  // and the text is still the one to show.
  for (const [errorKind, index] of [
    ['newer_kind', 1],
    [undefined, 2],
  ]) {
    const refused = service.setTarget('off');
    bridge.channels[index].reply({
      returnValue: false,
      errorKind,
      errorText: 'refused here',
    });
    await assert.rejects(refused, {
      kind: 'refused',
      errorKind: 'unknown',
      message: 'refused here',
    });
  }
  const removing = service.uninstall();
  assert.deepEqual(bridge.channels[3].payload, {});
  bridge.channels[3].reply({ returnValue: true });
  assert.equal(await removing, undefined);
  const enabling = service.setEnabled(false);
  bridge.channels[4].reply({ returnValue: true, unexpected: true });
  await assert.rejects(enabling, { kind: 'invalid_reply' });
  const thrown = lunaTvService(() => ({
    onservicecallback: null,
    cancel() {},
    call() {
      throw new Error('Bridge call failed');
    },
  }));
  await assert.rejects(thrown.setEnabled(true), {
    kind: 'unavailable',
    message: 'Bridge call failed',
  });
  await assert.rejects(lunaTvService(null).setEnabled(true), {
    kind: 'unavailable',
  });
  const pending = service.setEnabled(true);
  service.cancelAll();
  assert.equal(bridge.channels.at(-1).cancelled, 1);
  void pending.catch(() => {});
});

test('setup runs setup.sh through the Homebrew Channel and reports its stderr', async () => {
  const bridge = manualBridge();
  const service = lunaTvService(bridge.factory);
  const ok = service.setup();
  assert.equal(bridge.channels[0].uri, HOMEBREW_EXEC);
  assert.deepEqual(bridge.channels[0].payload, { command: SETUP_SCRIPT });
  assert.equal(SERVICE_URI, `luna://${APP_ID}.service`);
  assert.equal(
    SETUP_SCRIPT,
    `/media/developer/apps/usr/palm/services/${APP_ID}.service/setup.sh`,
  );
  bridge.channels[0].reply({ returnValue: true, stdoutString: '' });
  await ok;
  const failing = service.setup();
  bridge.channels[1].reply({
    returnValue: false,
    stderrString: 'setup: cannot write manifests',
  });
  await assert.rejects(failing, {
    kind: 'refused',
    message: 'setup: cannot write manifests',
  });
  const missing = service.setup();
  bridge.channels[2].reply({
    returnValue: false,
    errorText: 'Unknown service name: org.webosbrew.hbchannel.service',
  });
  await assert.rejects(missing, {
    kind: 'unavailable',
    message: /Homebrew Channel/,
  });
});

/* ---- Rules ---- */

test('addresses: Prysm addresses from what a user types', () => {
  assert.deepEqual(prysmAddress('192.168.1.20'), {
    value: 'http://192.168.1.20:8787',
  });
  assert.deepEqual(prysmAddress(' prysm.local/ '), {
    value: 'http://prysm.local:8787',
  });
  assert.deepEqual(prysmAddress('192.168.1.20:9000'), {
    value: 'http://192.168.1.20:9000',
  });
  assert.deepEqual(prysmAddress('HTTPS://prysm.home'), {
    value: 'https://prysm.home',
  });
  assert.deepEqual(prysmAddress('[fd00::2]'), {
    value: 'http://[fd00::2]:8787',
  });
  assert.deepEqual(prysmAddress(''), { value: null });
  assert('error' in prysmAddress('http://prysm.local/portal'));
  assert('error' in prysmAddress('ftp://prysm'));
  assert.equal(
    localPortal('http://192.168.1.5:8787/'),
    'http://127.0.0.1:8787',
  );
  assert.equal(localPortal(null), null);
});

test('status card covers the supervisor states for both packages', () => {
  const slim = (
    change = {},
    prefs = { prysm_address: 'http://192.168.1.20:8787' },
  ) =>
    tvState(
      'slim',
      { enabled: true, worker: { phase: 'running' }, ...change },
      prefs,
    );
  const draft = (value = {}) =>
    completePreferences({
      prysm_address: 'http://192.168.1.20:8787',
      ...value,
    });
  const title = (value, d = draft()) => tvStatus(live(value), d).title;
  assert.equal(tvStatus(connecting, draft()).title, 'Connecting…');
  assert.equal(tvStatus(missing, draft()).label, 'Set up service');
  // A state kept from before the service went missing is not shown.
  assert.equal(
    tvStatus({ status: 'missing', state: slim() }, draft()).label,
    'Set up service',
  );
  // A lost subscription keeps its last state.
  assert.equal(
    tvStatus({ status: 'lost', state: slim({ capturing: true }) }, draft())
      .title,
    'Sending the picture',
  );
  assert.equal(title(slim({ capturing: true })), 'Sending the picture');
  assert(tvStatus(live(slim({ capturing: true })), draft()).ready);
  assert.equal(title(slim({ power: 'asleep' })), 'TV is asleep');
  assert.equal(title(slim({ power: 'screen_saver' })), 'Screen saver is on');
  assert.equal(title(slim({ video: 'unavailable' })), 'Waiting for video');
  const waiting = tvStatus(
    live(
      slim({
        uplink: {
          state: 'waiting',
          message:
            'waiting for the receiver: cannot connect to receiver: refused',
        },
      }),
    ),
    draft(),
  );
  assert.equal(waiting.title, 'Waiting for Prysm');
  assert.equal(waiting.attention, false, 'waiting is not a failure');
  const failedSlim = tvStatus(
    live(slim(asking(ask('try_again'), 'worker exited with code 1'))),
    draft(),
  );
  assert.deepEqual(
    [
      failedSlim.title,
      failedSlim.description,
      failedSlim.attention,
      failedSlim.label,
    ],
    [
      'Capture stopped',
      'Stop and start capture again to retry.',
      true,
      'Stop capture',
    ],
    'plain words: the message is for Diagnostics',
  );
  assert.doesNotMatch(failedSlim.description, /worker/);
  const stopping = tvStatus(
    live(slim({ worker: { phase: 'stopping' } })),
    draft(),
  );
  assert.deepEqual(
    [stopping.title, stopping.label, stopping.action],
    ['Stopping capture…', 'Please wait…', 'none'],
  );
  // A stop that will ask says what, not that capture stops.
  const stoppingToAsk = tvStatus(
    live(
      slim({
        worker: { phase: 'stopping' },
        problem: { asked: ask('try_again'), message: 'x' },
      }),
    ),
    draft(),
  );
  assert.deepEqual(
    [stoppingToAsk.title, stoppingToAsk.label],
    ['Capture stopped', 'Please wait…'],
  );
  const notSet = tvStatus(
    live(slim({ enabled: false }, {})),
    completePreferences({}),
  );
  assert.deepEqual(
    [notSet.title, notSet.label, notSet.action],
    ['Set up this TV', 'Add Prysm address', 'address'],
  );
  assert.equal(
    tvStatus(live(slim({ enabled: false })), draft()).label,
    'Start capture',
  );
  assert.equal(tvStatus(live(slim()), draft()).label, 'Stop capture');
  assert.equal(title(slim({ enabled: false })), 'Capture off');
  const full = (change = {}) =>
    tvState('full', {
      target: 'screen_sync',
      worker: { phase: 'running' },
      ...change,
    });
  const lightsOff = tvStatus(live(full({ target: 'off' })), draft());
  assert.deepEqual(
    [lightsOff.title, lightsOff.label, lightsOff.action],
    ['Lights off', '', 'none'],
    'the lighting modes start screen sync; no second button',
  );
  assert.equal(title(full({ capturing: true })), 'Screen sync on');
  assert.equal(title(full({ target: 'effects' })), 'WLED effects on');
  // Screen sync runs, but the lights don't show it: said over the picture,
  // with the portal's headline; the lights card says why. Only lights that
  // don't answer come back by themselves.
  const pointer = 'See Your lights for what to do.';
  for (const [lights, heading, detail] of [
    [
      'unreachable',
      'Can’t reach the lights',
      'The lights follow again as soon as they answer.',
    ],
    ['off', 'Not showing screen sync', pointer],
    ['overridden', 'Not showing screen sync', pointer],
    ['not_live', 'Not showing screen sync', pointer],
    ['other_sender', 'Not showing screen sync', pointer],
  ]) {
    const status = tvStatus(live(full({ capturing: true, lights })), draft());
    assert.deepEqual(
      [status.title, status.description, status.attention, status.action],
      [heading, detail, true, 'none'],
      lights,
    );
  }
  // Once the portal's state is known, its issue is the one said, as on the
  // lights card (none included); before, the worker's.
  const syncing = (issue) => ({
    lights: true,
    address: null,
    lighting: { kind: 'sync', input: undefined },
    issue,
  });
  const unreachable = live(full({ capturing: true, lights: 'unreachable' }));
  assert.equal(
    tvStatus(unreachable, draft(), syncing('off')).description,
    pointer,
  );
  assert.equal(
    tvStatus(unreachable, draft(), syncing(null)).title,
    'Screen sync on',
  );
  // With the lights off or their effects, lights that don't answer are said
  // too, with the same headline: the target stays and is applied once they
  // answer. By the worker's word, then by the portal's.
  for (const [target, detail, answered] of [
    [
      'off',
      'The lights switch off as soon as they answer.',
      { kind: 'off', failure: null },
    ],
    [
      'effects',
      'WLED effects start as soon as the lights answer.',
      { kind: 'effects', failure: null },
    ],
  ]) {
    const waited = live(full({ target, lights: 'unreachable' }));
    const said = (portal) => {
      const status = tvStatus(waited, draft(), portal);
      return [status.title, status.description, status.attention];
    };
    const expected = ['Can’t reach the lights', detail, true];
    const facts = (lighting, issue) => ({
      lights: true,
      address: null,
      lighting,
      issue,
    });
    assert.deepEqual(said(), expected, target);
    assert.equal(tvStatus(waited, draft()).action, 'none', target);
    assert.deepEqual(
      said(facts({ kind: 'unanswered', target }, 'unreachable')),
      expected,
      target,
    );
    // Once the portal says they answered, the target's own words.
    assert.notEqual(said(facts(answered, null))[0], expected[0], target);
  }
  // Any other issue the worker still says is screen sync's alone.
  assert.equal(
    tvStatus(live(full({ target: 'off', lights: 'not_live' })), draft()).title,
    'Lights off',
  );
  const noFrames = tvStatus(live(full({ capturing: false })), draft());
  assert.equal(
    noFrames.title,
    'Waiting for video',
    'lighting on without frames',
  );
  assert.equal(
    noFrames.description,
    'Screen sync starts when a picture arrives.',
  );
  assert.equal(noFrames.attention, false);
  // Slim: the TV refuses to share the video. Not a failure, and it ends by
  // itself; slim keeps its usual Stop capture. (Full hears it from its
  // portal.)
  const refusedSlim = tvStatus(live(slim({ refused: true })), draft());
  assert.deepEqual(
    [
      refusedSlim.title,
      refusedSlim.description,
      refusedSlim.attention,
      refusedSlim.action,
    ],
    [
      'Video can’t be captured',
      'Capture continues when the video changes.',
      false,
      'disable',
    ],
  );
  assert.equal(
    title(full({ capturing: false, portal: null })),
    'Starting screen sync…',
  );
  assert.equal(
    tvStatus(live(full({ video: 'unavailable' })), draft()).description,
    'Choose an input or play something on the TV.',
  );
  // Every headline and detail fits its one line (about 52 characters).
  const all = [
    tvStatus(connecting, draft()),
    tvStatus(missing, draft()),
    ...[
      slim(),
      slim({ capturing: true }),
      slim({ paused: true }),
      slim({ enabled: false }),
      slim({ power: 'asleep' }),
      slim({ video: 'unavailable' }),
      ...[
        'waiting',
        'encoder_failed',
        'version_differs',
        'replaced',
        'receiver_failed',
      ].map((state) => slim({ uplink: { state, message: 'x' } })),
      slim({ unreadable: 'x' }),
      full({ target: 'off' }),
      full({ capturing: true }),
      full({ target: 'effects' }),
      full({ capturing: true, lights: 'unreachable' }),
      full({ capturing: true, lights: 'not_live' }),
      full({ target: 'off', lights: 'unreachable' }),
      full({ target: 'effects', lights: 'unreachable' }),
      slim({ refused: true }),
      full({ unreadable: 'x' }),
      ...everyAsked.flatMap((asked) => [
        slim(asking(asked)),
        full(asking(asked)),
        ...['off', 'screen_sync', 'effects'].map((target) =>
          full({ target, problem: { asked, message: 'x' } }),
        ),
      ]),
    ].map((value) => tvStatus(live(value), draft())),
    tvStatus(live(full({})), draft(), { lights: false, address: null }),
    // Every headline the portal's lighting gives what is asked.
    ...[
      { state: 'stopped', failure: { kind: 'adapter', message: 'x' } },
      { state: 'effects', failure: { kind: 'adapter', message: 'x' } },
      { state: 'stopped' },
      { state: 'effects' },
      { state: 'screen_sync' },
    ].flatMap((session) =>
      [{}, { problem: { asked: ask('try_again'), message: 'x' } }].map(
        (asked) =>
          tvStatus(live(full({ target: 'screen_sync', ...asked })), draft(), {
            lights: true,
            address: null,
            lighting: lightingSituation({ target: 'screen_sync', session }),
          }),
      ),
    ),
  ];
  for (const { title: heading, description } of all) {
    assert(heading.length <= 24, heading);
    assert(description.length <= 52, description);
    assert.doesNotMatch(`${heading} ${description}`, /https?:|\d+ s\b/);
  }
  // Unreadable saved preferences: nothing stopped, so nothing to retry; a
  // saved setting replaces them.
  for (const value of [full({ unreadable: 'x' }), slim({ unreadable: 'x' })]) {
    const unreadable = tvStatus(live(value), draft());
    assert.equal(unreadable.title, 'Couldn’t read settings');
    assert.equal(unreadable.attention, true);
    assert.notEqual(unreadable.action, 'retry');
  }
  assert.equal(tvStatus(live(full()), draft()).action, 'none');
  const failedFull = tvStatus(live(full(asking(ask('try_again')))), draft());
  assert.deepEqual(
    [failedFull.label, failedFull.action],
    ['Try again', 'retry'],
    'after a failure the service is restarted, not the lights chosen',
  );
});

test('what the service asks has its words and its button in each package', () => {
  const draft = completePreferences({
    prysm_address: 'http://192.168.1.20:8787',
  });
  const status = (flavor, facts) =>
    tvStatus(
      live(
        tvState(
          flavor,
          flavor === 'full'
            ? { target: 'screen_sync', ...facts }
            : { enabled: true, ...facts },
          flavor === 'full'
            ? {}
            : { prysm_address: 'http://192.168.1.20:8787' },
        ),
      ),
      draft,
    );
  const said = (flavor, asked) => {
    const { title, description, attention, label } = status(
      flavor,
      asking(asked),
    );
    assert(attention, JSON.stringify(asked));
    return [title, description, label];
  };
  const reach = 'Can’t reach the lights';
  assert.deepEqual(
    everyAsked.map((asked) => said('full', asked)),
    [
      [reach, 'Prysm stopped trying. Check them, then try again.', 'Try again'],
      [
        'Screen sync stopped',
        'Try again, or choose another mode.',
        'Try again',
      ],
      ['Screen sync stopped', 'Change the settings in the web portal.', ''],
      [
        'Prysm needs setting up',
        'Set up its service on this TV to start.',
        'Set up service',
      ],
      [
        'Capture not supported',
        'This TV doesn’t let Prysm capture its picture.',
        '',
      ],
      [
        'Restart the TV',
        'Prysm can’t capture again until the TV restarts.',
        '',
      ],
    ],
  );
  assert.deepEqual(
    everyAsked.map((asked) => said('slim', asked)),
    [
      [
        reach,
        'Prysm stopped trying. Stop and start capture again.',
        'Stop capture',
      ],
      [
        'Capture stopped',
        'Stop and start capture again to retry.',
        'Stop capture',
      ],
      [
        'Capture stopped',
        'Change the settings in Prysm’s web portal.',
        'Stop capture',
      ],
      [
        'Prysm needs setting up',
        'Set up its service on this TV to start.',
        'Set up service',
      ],
      [
        'Capture not supported',
        'This TV doesn’t let Prysm capture its picture.',
        'Stop capture',
      ],
      [
        'Restart the TV',
        'Prysm can’t capture again until the TV restarts.',
        'Stop capture',
      ],
    ],
  );
  // Full: the problem is said with the lights off too; with no worker,
  // Prysm itself stopped, and trying again sends the target kept.
  const off = status('full', { target: 'off', ...asking(ask('try_again')) });
  assert.deepEqual(
    [off.title, off.description, off.action],
    ['Prysm stopped', 'Try again, or choose another mode.', 'retry'],
  );
  assert.equal(
    status('full', asking(ask('try_again'))).title,
    'Screen sync stopped',
  );
  assert.equal(
    status('full', { target: 'effects', ...asking(ask('try_again')) }).title,
    'WLED effects stopped',
  );
  // A running full worker asks too, under what the lights do.
  const running = status('full', {
    target: 'off',
    problem: { asked: ask('try_again'), message: 'x' },
  });
  assert.deepEqual([running.title, running.action], ['Lights off', 'retry']);
  // A stop that will ask says it, not a restart; one that restarts says so.
  const stopping = (problem) =>
    status('full', { worker: { phase: 'stopping' }, problem });
  assert.equal(stopping(null).title, 'Restarting Prysm…');
  assert.deepEqual(
    [
      stopping({ asked: ask('set_up'), message: 'x' }).title,
      stopping({ asked: ask('set_up'), message: 'x' }).action,
    ],
    ['Prysm needs setting up', 'none'],
  );
  // Nothing to send for a target newer than this app.
  assert.equal(
    status('full', { target: 'unknown', ...asking(ask('try_again')) }).action,
    'none',
  );
  // Diagnostics has what is asked, in its own words.
  const text = diagnostics(tvState('full', asking(ask('set_up'), 'denied')));
  assert.match(text, /Background process: stopped until you choose what to do/);
  assert.match(text, /denied \(asked: set_up\)/);
  assert.match(
    diagnostics(tvState('full', asking(UNANSWERED, 'no answer'))),
    /no answer \(asked: unanswered\)/,
  );
  assert.match(
    diagnostics(tvState('full', { unreadable: 'invalid tv.json' })),
    /Saved settings unreadable: invalid tv\.json/,
  );
});

test('slim encoder failures point to the portal; waits, other failures and the full package do not', () => {
  const slim = (change = {}) =>
    tvState(
      'slim',
      { enabled: true, worker: { phase: 'running' }, ...change },
      { prysm_address: 'http://192.168.1.20:8787' },
    );
  const draft = completePreferences({
    prysm_address: 'http://192.168.1.20:8787',
  });
  const status = (value) => tvStatus(live(value), draft);
  const hint = 'Choose another encoding in Prysm’s web portal.';
  assert.deepEqual(
    ['h264_hardware', 'h264_software', 'raw'].map(encodingName),
    ['Hardware H.264', 'Software H.264', 'Uncompressed'],
  );
  assert.equal(
    'changeEncoding' in status(slim()),
    false,
    'the TV offers no encoding switch',
  );
  const failed = status(
    slim({
      uplink: {
        state: 'encoder_failed',
        message: 'hardware H.264 encoder cannot start',
      },
    }),
  );
  assert.deepEqual(
    [failed.title, failed.description],
    ['Encoding not supported', hint],
  );
  assert(failed.attention);
  const other = status(
    slim(
      asking(ask('try_again'), 'worker failed: the encoder of capture lost'),
    ),
  );
  assert.deepEqual(
    [other.title, other.description],
    ['Capture stopped', 'Stop and start capture again to retry.'],
    'only the structured state counts, not the text',
  );
  const paused = status(slim({ paused: true, encoding: 'h264_hardware' }));
  assert.deepEqual(
    [paused.title, paused.attention, paused.ready],
    ['Paused by Prysm', false, false],
  );
  assert.equal(
    paused.description,
    'It continues when Prysm turns the lights on.',
  );
  assert.match(
    diagnostics(slim({ paused: true })),
    /Capturing: no, paused by Prysm \(lights off on the computer\)/,
  );
  assert.match(
    diagnostics(slim({ refused: true })),
    /Capturing: no\. The TV doesn’t let Prysm capture this video\./,
  );
  assert.match(diagnostics(slim()), /Capturing: no\n/);
  const wait = status(
    slim({
      uplink: {
        state: 'waiting',
        message: 'waiting for the receiver: refused',
      },
    }),
  );
  assert.deepEqual([wait.title, wait.attention], ['Waiting for Prysm', false]);
  assert.doesNotMatch(wait.description, /encoding/);
  // Another stream version is not a wait for Prysm: the older side needs
  // updating.
  const versions = status(
    slim({
      uplink: {
        state: 'version_differs',
        message:
          'waiting for the receiver: the capture stream versions differ (Prysm: 9, this end: 6); update the older one',
      },
    }),
  );
  assert.deepEqual(
    [versions.title, versions.description, versions.attention],
    ['Update Prysm or TV app', 'Update the older of Prysm and this app.', true],
  );
  // Another TV took the stream over: this one stopped until capture is
  // started again, which the button offers.
  const replaced = status(
    slim({
      uplink: {
        state: 'replaced',
        message:
          'the stream to ws://prysm.local:8787/api/v1/capture/stream failed: another sender replaced this one at Prysm',
      },
    }),
  );
  assert.deepEqual(
    [replaced.title, replaced.description, replaced.attention, replaced.action],
    [
      'Another TV took over',
      'Stop and start capture to send from this TV.',
      true,
      'disable',
    ],
  );
  // Prysm's source failed with this TV's stream: not paused, not a lights
  // off on the computer; its portal says why.
  const receiverFailed = status(
    slim({
      uplink: {
        state: 'receiver_failed',
        message:
          'the stream to ws://prysm.local:8787/api/v1/capture/stream failed: Prysm stopped receiving after a failure, which it shows',
      },
    }),
  );
  assert.deepEqual(
    [
      receiverFailed.title,
      receiverFailed.description,
      receiverFailed.attention,
    ],
    [
      'Prysm stopped receiving',
      'Its web portal on your computer says why.',
      true,
    ],
  );
  const versionText = status(
    slim({
      uplink: {
        state: 'waiting',
        message: 'the capture stream versions differ; update the older one',
      },
    }),
  );
  assert.equal(
    versionText.title,
    'Waiting for Prysm',
    'only the state says the versions differ, not the text',
  );
  const unknownWait = status(
    slim(asking(ask('try_again'), 'waiting for the receiver: refused')),
  );
  assert.equal(
    unknownWait.title,
    'Capture stopped',
    'the text prefix no longer decides',
  );
  const full = tvState('full', asking(ask('try_again'), 'encoder lost'));
  assert.doesNotMatch(status(full).description, /encoding/);
  assert.match(
    diagnostics(slim({ encoding: 'raw' })),
    /Stream encoding: Uncompressed in use/,
  );
  assert.match(diagnostics(slim()), /Stream encoding: none in use/);
});

test('the capture rate is shown while the worker measures it', async (t) => {
  const measured = (fps) => tvState('full', { capturing: true, fps });
  assert.equal(captureRate(measured(30)), '30 frames per second');
  assert.equal(captureRate(measured(0)), 'No frames yet');
  assert.equal(captureRate(measured(null)), null);
  assert.equal(
    captureRate(tvState('full', { worker: { phase: 'stopping' }, fps: 30 })),
    null,
    'only a running worker measures',
  );
  assert.match(diagnostics(measured(30)), /Capture rate: 30 frames per second/);
  assert.match(diagnostics(measured(null)), /Capture rate: not measured/);
  const tv = await mountTv(t, {
    supervisor: {
      preferences: { prysm_address: 'http://192.168.1.20:8787' },
      state: {
        enabled: true,
        worker: { phase: 'running' },
        capturing: true,
        fps: 30,
        encoding: 'raw',
      },
    },
  });
  // A sending slim link always names its encoding.
  assert.equal(tv.text('status-meta'), '30 frames per second · Uncompressed');
  await act(async () => tv.supervisor.publish({ capturing: false, fps: null }));
  await settle();
  // The line stays (nothing moves), empty.
  assert.equal(tv.text('status-meta'), '\u00a0');
});

test('preferences are completed with their defaults', () => {
  assert.deepEqual(completePreferences({ autostart: true }), {
    autostart: true,
    prysm_address: null,
  });
  // No address is omitted on the wire, never sent as `null`.
  assert.deepEqual(sentPreferences({ autostart: true, prysm_address: null }), {
    autostart: true,
  });
});

test('the service link: each event in each link state', () => {
  // A failed setup's notice was cleared by every failed resubscription.
  const full = fixtures['luna/State'][0];
  const withLink = (link) => ({ ...initialService, link, notice: 'Kept.' });
  const states = {
    connecting: withLink({ status: 'connecting' }),
    live: withLink({ status: 'live', state: full }),
    lost: withLink({ status: 'lost', state: full }),
    missing: withLink({ status: 'missing', state: null }),
  };
  const offline = (kind) => ({
    type: 'event',
    event: { online: false, error: { kind, message: kind } },
  });
  const events = {
    online: { type: 'event', event: { online: true, state: full } },
    unavailable: offline('unavailable'),
    refused: offline('refused'),
    removed: { type: 'removed' },
  };
  // The link and the notice after each event.
  const table = {
    connecting: {
      online: ['live', 'Kept.'],
      unavailable: ['missing', ''],
      refused: ['missing', ''],
      removed: ['missing', ''],
    },
    live: {
      online: ['live', 'Kept.'],
      unavailable: ['missing', ''],
      refused: ['lost', 'Kept.'],
      removed: ['missing', ''],
    },
    lost: {
      online: ['live', 'Kept.'],
      unavailable: ['missing', ''],
      refused: ['missing', ''],
      removed: ['missing', ''],
    },
    missing: {
      online: ['live', 'Service connected.'],
      unavailable: ['missing', 'Kept.'],
      refused: ['missing', 'Kept.'],
      removed: ['missing', 'Kept.'],
    },
  };
  for (const [name, state] of Object.entries(states))
    for (const [kind, event] of Object.entries(events)) {
      const next = serviceReducer(state, event);
      assert.deepEqual(
        [next.link.status, next.notice],
        table[name][kind],
        `${name} × ${kind}`,
      );
      // The last state stays, whatever the link does.
      if (name === 'live' || name === 'lost')
        assert.equal(
          next.link.state,
          full,
          `${name} × ${kind} keeps the state`,
        );
      if (event.type === 'event' && !event.event.online)
        assert.equal(next.problem, kind, 'its words go to Diagnostics');
    }
  // The rest leave the link as it is.
  const saved = { autostart: true, prysm_address: null };
  for (const state of Object.values(states))
    for (const [action, check] of [
      [{ type: 'saved', stored: saved }, (next) => next.pending !== undefined],
      [{ type: 'changing', value: true }, (next) => next.changing === true],
      [
        { type: 'notice', text: 'Done.', error: true },
        (next) => next.notice === 'Done.' && next.error === true,
      ],
      [{ type: 'problem', text: 'x' }, (next) => next.problem === 'x'],
    ]) {
      const next = serviceReducer(state, action);
      assert.equal(next.link, state.link, action.type);
      assert(check(next), action.type);
    }
});

test('a pending save ends at the first push that shows anything else', () => {
  const full = fixtures['luna/State'][0];
  const withPreferences = (preferences) => ({ ...full, preferences });
  const off = { autostart: false, prysm_address: null };
  const on = { autostart: true, prysm_address: null };
  const push = (preferences) => ({
    type: 'event',
    event: { online: true, state: withPreferences(preferences) },
  });
  const live = {
    ...initialService,
    link: { status: 'live', state: withPreferences(off) },
  };
  const saved = serviceReducer(live, { type: 'saved', stored: on });
  assert.deepEqual(saved.pending, { stored: on, before: off });
  // An older push, sent before the save, keeps it shown.
  assert.deepEqual(serviceReducer(saved, push(off)).pending, saved.pending);
  // The saved preferences end it, and so does a change made since.
  assert.equal(serviceReducer(saved, push(on)).pending, null);
  const elsewhere = {
    autostart: false,
    prysm_address: 'https://prysm.example.com',
  };
  assert.equal(serviceReducer(saved, push(elsewhere)).pending, null);
  // Already shown: nothing is pending.
  assert.equal(
    serviceReducer(live, { type: 'saved', stored: off }).pending,
    null,
  );
});

test('the main screen: one place says a package mismatch and what shows', () => {
  const slim = tvState(
    'slim',
    {
      enabled: true,
      worker: { phase: 'running' },
      capturing: true,
      fps: 30,
      encoding: 'raw',
    },
    { prysm_address: 'http://192.168.1.20:8787' },
  );
  const draft = completePreferences({
    prysm_address: 'http://192.168.1.20:8787',
  });
  const screen = (change = {}) =>
    tvScreen({
      flavor: 'slim',
      status: tvStatus(live(slim), draft),
      state: slim,
      changing: false,
      portalAddress: 'http://192.168.1.20:8787',
      ...change,
    });
  const sending = screen();
  assert.deepEqual(
    [sending.title, sending.meta, sending.label, sending.showPrimary],
    [
      'Sending the picture',
      '30 frames per second · Uncompressed',
      'Stop capture',
      true,
    ],
  );
  assert.equal(sending.mismatch, false);
  assert.equal(sending.portalPanel, true);
  const wrong = screen({ flavor: 'full' });
  assert.deepEqual(
    [
      wrong.title,
      wrong.description,
      wrong.attention,
      wrong.mismatch,
      wrong.meta,
    ],
    [
      'Reinstall Prysm',
      'The app and its TV service don’t match.',
      true,
      true,
      '',
    ],
  );
  assert.deepEqual(
    [screen({ changing: true }).label, screen({ changing: true }).waiting],
    ['Please wait…', true],
  );
});

/* ---- Screens ---- */

test('an unavailable service offers setup, keeps the error visible and connects after setup', async (t) => {
  const tv = await mountTv(t, { supervisor: { offline: true } });
  assert.equal(tv.text('status-title'), 'Prysm needs setting up');
  assert.equal(tv.text('primary'), 'Set up service');
  // The service's own words are for Diagnostics, not the screen.
  assert.equal(tv.text('notice'), '');
  assert.equal(tv.focus(), 'primary');
  await tv.click('primary');
  await settle();
  const exec = tv.supervisor.calls.find((call) =>
    call.uri.startsWith('luna://org.webosbrew'),
  );
  assert.deepEqual(exec.payload, { command: SETUP_SCRIPT });
  assert.equal(tv.text('status-title'), 'Set up this TV');
});

test('every state keeps the same slots, plain words and focus on a control', async (t) => {
  for (const flavor of ['full', 'slim']) {
    const tv = await mountTv(t, {
      flavor,
      supervisor: flavor === 'slim' ? { preferences: linked } : {},
    });
    // Lighting (full) or capture (slim), in the package's own terms.
    const [on, off] =
      flavor === 'full'
        ? [{ target: 'screen_sync' }, { target: 'off' }]
        : [{ enabled: true }, { enabled: false }];
    const running = { worker: { phase: 'running' }, ...on };
    const states = [
      [
        'off',
        {
          ...off,
          worker: { phase: flavor === 'full' ? 'running' : 'stopped' },
        },
      ],
      ['on', { ...running, capturing: true, fps: 30 }],
      [
        'waiting for video',
        { ...running, capturing: false, fps: null, video: 'unavailable' },
      ],
      ['asleep', { ...running, video: 'available', power: 'asleep' }],
      ['screen saver', { ...running, power: 'screen_saver' }],
      [
        'asking',
        {
          ...running,
          power: 'active',
          ...asking(
            ask('try_again'),
            'worker failed: stderr: panic at src/main.rs',
          ),
        },
      ],
      [
        'stopping to ask',
        {
          worker: { phase: 'stopping' },
          problem: {
            asked: ask('set_up'),
            message: 'again after a restart: worker exited with code 101',
          },
        },
      ],
      ...(flavor === 'slim'
        ? [
            [
              'receiver',
              {
                ...running,
                problem: null,
                uplink: {
                  state: 'waiting',
                  message: 'Connection refused (os error 111)',
                },
              },
            ],
            ['paused', { ...running, uplink: null, paused: true }],
            [
              'encoder',
              {
                ...running,
                paused: false,
                uplink: { state: 'encoder_failed', message: 'v4l2: EBUSY' },
              },
            ],
            [
              'version',
              {
                ...running,
                uplink: {
                  state: 'version_differs',
                  message:
                    'waiting for the receiver: Prysm answered HTTP 426: versions differ',
                },
              },
            ],
            [
              'replaced',
              {
                ...running,
                uplink: {
                  state: 'replaced',
                  message: 'another sender replaced this one at Prysm',
                },
              },
            ],
          ]
        : []),
      // A phase newer than this app: nothing to start or stop.
      [
        'updating',
        {
          worker: { phase: 'unknown' },
          problem: null,
          ...(flavor === 'slim' ? { uplink: null } : {}),
        },
      ],
      ['stopping', { worker: { phase: 'stopping' } }],
      ['back', { ...running, capturing: true, fps: 30 }],
    ];
    for (const [name, change] of states) {
      await act(async () => tv.supervisor.publish(change));
      await settle();
      const label = `${flavor} ${name}`;
      // The slots are always there, so nothing moves between states.
      for (const id of [
        'status-title',
        'status-description',
        'status-meta',
        'lighting-status',
      ])
        assert(tv.el(id), `${label}: #${id}`);
      assert(tv.text('status-title'), `${label}: a headline`);
      assert(tv.text('status-description'), `${label}: one line of detail`);
      assert(tv.text('status-title').length <= 24, label);
      assert(tv.text('status-description').length <= 52, label);
      if (name === 'version')
        assert.deepEqual(
          [tv.text('status-title'), tv.text('status-description')],
          ['Update Prysm or TV app', 'Update the older of Prysm and this app.'],
          'another stream version asks for an update, not for Prysm to run',
        );
      // No programmer words outside Diagnostics.
      assert.doesNotMatch(
        tv.el('app').textContent,
        /worker|stderr|panic|os error|EBUSY|v4l2|exited|null|origin|Luna/i,
        label,
      );
      // Focus is always on a usable control.
      const active = document.activeElement;
      assert(
        active && active !== document.body && !active.disabled,
        `${label}: focus`,
      );
      if (name === 'stopping' && flavor === 'slim') {
        // Busy, not disabled: focus stays put and OK does nothing.
        assert.equal(tv.el('primary').getAttribute('aria-busy'), 'true');
        assert.equal(tv.focus(), 'primary');
        const calls = tv.supervisor.calls.length;
        await tv.click('primary');
        assert.equal(tv.supervisor.calls.length, calls, 'OK does nothing');
      }
      if (name === 'stopping' && flavor === 'full')
        assert(!tv.el('primary'), 'no button while the service has no task');
      if (name === 'updating') {
        assert.equal(tv.text('status-title'), 'Updating…', label);
        assert(!tv.el('primary'), `${label}: nothing to press`);
        assert(
          document.querySelector('.primary-slot'),
          `${label}: the button's place is kept`,
        );
      }
      if (name === 'back')
        assert.equal(
          tv.focus(),
          flavor === 'slim' ? 'primary' : 'mode-off',
          'focus returns once it can',
        );
    }
    await tv.unmount();
  }
});

test('remote arrows move focus through rows and columns without changing settings', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
      },
    },
  });
  assert.equal(tv.focus(), 'primary');
  await tv.key(DOWN);
  assert.equal(
    tv.focus(),
    'mode-off',
    'the lighting row is one stop, entered at its chosen mode',
  );
  await tv.key(LEFT);
  assert.equal(tv.focus(), 'mode-effects');
  await tv.key(RIGHT);
  await tv.key(RIGHT);
  assert.equal(
    tv.focus(),
    'prysm-address',
    'the row edge continues to the settings column',
  );
  await tv.key(DOWN);
  assert.equal(tv.focus(), 'autostart');
  await tv.key(DOWN);
  assert.equal(
    tv.focus(),
    'service-options',
    'an unavailable row is passed over',
  );
  await tv.key(UP);
  await tv.key(LEFT);
  assert.equal(
    tv.focus(),
    'mode-off',
    'left returns to the control last used there',
  );
  await tv.key(RIGHT);
  assert.equal(tv.focus(), 'autostart');
  await tv.key(UP);
  await tv.key(UP);
  await tv.key(UP);
  assert.equal(tv.focus(), 'prysm-address');
  assert(!tv.methods().includes('setPreferences'), 'arrows change nothing');
});

test('full: without a task for the service there is no primary button; focus starts on the chosen mode', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  assert(!tv.el('primary'));
  assert.equal(tv.focus(), 'mode-off');
  await tv.key(RIGHT);
  assert.equal(tv.focus(), 'autostart', 'the first row of the settings');
});

test('Start with TV is a switch: OK flips it and it applies at once', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
      },
    },
  });
  tv.el('autostart').focus();
  assert.equal(tv.el('autostart').getAttribute('role'), 'switch');
  assert.equal(tv.el('autostart').getAttribute('aria-checked'), 'false');
  await tv.key(LEFT);
  await tv.key(RIGHT);
  assert(!tv.methods().includes('setPreferences'), 'arrows change nothing');
  await tv.click('autostart');
  await settle();
  assert(!tv.el('modal'), 'no dialog for an on/off value');
  const saved = tv.supervisor.calls.find(
    (call) => call.method === 'setPreferences',
  ).payload;
  assert.equal(saved.autostart, true);
  assert.equal(
    saved.prysm_address,
    'http://192.168.1.20:8787',
    'the rest is kept',
  );
  assert.equal(tv.el('autostart').getAttribute('aria-checked'), 'true');
  assert.equal(tv.text('autostart-value'), 'On');
  assert.equal(
    tv.text('notice'),
    '',
    'success says nothing; failures are reported',
  );
  assert.equal(tv.supervisor.state().preferences.autostart, true);
  assert(!tv.el('save'), 'nothing waits for a Save');
});

test('the Prysm address is explicit: invalid text is explained and Back keeps the old one', async (t) => {
  const tv = await mountTv(t);
  assert.equal(tv.text('primary'), 'Add Prysm address');
  await tv.click('primary');
  assert.equal(tv.text('dialog-title'), 'Prysm address');
  assert.equal(tv.focus(), 'prysm-address-input');
  await tv.key(LEFT);
  assert.equal(
    tv.focus(),
    'prysm-address-input',
    'left and right move the caret',
  );
  await tv.type('prysm-address-input', 'http://prysm.local/x');
  await tv.submit('prysm-address-input');
  assert.equal(
    tv.text('prysm-address-input-error'),
    'That doesn’t look like an address. Try one like 192.168.1.20.',
  );
  assert.equal(
    tv.el('prysm-address-input').getAttribute('aria-describedby'),
    'prysm-address-input-error',
  );
  assert.match(tv.el('modal').className, /\btop\b/, 'clear of the keyboard');
  await tv.type('prysm-address-input', ' 192.168.1.30 ');
  await tv.key(BACK);
  assert.equal(tv.text('prysm-address-value'), 'Add');
  assert(!tv.methods().includes('setPreferences'), 'Back sends nothing');
  await tv.click('prysm-address');
  assert.equal(tv.el('prysm-address-input').value, '');
  await tv.type('prysm-address-input', '192.168.1.30');
  await tv.submit('prysm-address-input');
  await settle();
  assert(!tv.el('modal'));
  assert.equal(tv.text('prysm-address-value'), '192.168.1.30:8787');
  assert.equal(
    tv.supervisor.state().preferences.prysm_address,
    'http://192.168.1.30:8787',
    'Use this address saves it',
  );
  await tv.click('prysm-address');
  assert.equal(
    tv.el('prysm-address-input').value,
    '192.168.1.30:8787',
    'edited as it is shown',
  );
});

test('saving a Prysm address checks that a portal answers there, and only warns', async (t) => {
  const save = async (options) => {
    const tv = await mountTv(t, options);
    await tv.click('primary');
    await tv.type('prysm-address-input', '192.168.1.30');
    await tv.submit('prysm-address-input');
    await settle();
    assert.equal(
      tv.supervisor.state().preferences.prysm_address,
      'http://192.168.1.30:8787',
      'saved anyway',
    );
    const notice = tv.text('notice');
    await tv.unmount();
    return notice;
  };
  assert.equal(
    await save({ block: true }),
    'Saved. No Prysm answers at that address yet. Check that Prysm is running and the address is right.',
  );
  const refused =
    'Saved. Prysm answers there but doesn’t take commands from the TV; capture still works. The TV guide shows how to allow it.';
  assert.equal(
    await save({
      block: { kind: 'forbidden', message: 'origin denied' },
    }),
    refused,
  );
  // A CORS refusal is a network error; the probe still gets an answer.
  assert.equal(
    await save({ block: true, probe: () => Promise.resolve(true) }),
    refused,
  );
  assert.equal(await save({}), '', 'a portal answers: nothing to report');
});

test('a Prysm address is all the slim TV needs; then Start capture', async (t) => {
  const tv = await mountTv(t);
  await tv.click('primary');
  await tv.type('prysm-address-input', '192.168.1.20');
  await tv.submit('prysm-address-input');
  await settle();
  // The lights and the address check both use the one address.
  assert.deepEqual([...new Set(tv.urls)], ['http://192.168.1.20:8787']);
  assert(!tv.el('receiver'), 'no second address');
  assert.equal(tv.text('primary'), 'Start capture');
  await tv.click('primary');
  await settle();
  assert.deepEqual(
    tv.methods().filter((m) => m !== 'getState'),
    ['setPreferences', 'setEnabled'],
  );
  assert.equal(
    tv.supervisor.state().preferences.prysm_address,
    'http://192.168.1.20:8787',
  );
  assert.equal(tv.text('primary'), 'Stop capture');
});

test('a failed change keeps the saved value, says so, and never enables', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
      },
    },
  });
  tv.supervisor.failNext(
    'setPreferences',
    'invalid setPreferences request: Prysm address `x`: needs http:// or https://',
  );
  await tv.click('autostart');
  await settle();
  assert.equal(
    tv.text('notice'),
    'Couldn’t save your settings. Service options › Diagnostics has the details.',
  );
  // The service's own words are in Diagnostics.
  tv.el('service-options').focus();
  await tv.click('service-options');
  await tv.click('diagnostics');
  assert.match(
    tv.text('diagnostics-text'),
    /Last command failed: invalid setPreferences request: Prysm address `x`/,
  );
  await tv.key(BACK);
  await tv.key(BACK);
  assert(!tv.methods().includes('setEnabled'));
  assert.equal(tv.text('autostart-value'), 'Off', 'the saved value stays');
  assert.equal(tv.el('autostart').getAttribute('aria-checked'), 'false');
});

test('status updates keep focus; a change made elsewhere shows at once', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
      },
    },
  });
  tv.el('prysm-address').focus();
  await act(async () =>
    tv.supervisor.publish({
      capturing: true,
      enabled: true,
      worker: { phase: 'running' },
    }),
  );
  await settle();
  assert.equal(tv.focus(), 'prysm-address');
  const prefs = tv.supervisor.state().preferences;
  await act(async () =>
    tv.supervisor.publish({ preferences: { ...prefs, autostart: true } }),
  );
  await settle();
  assert.equal(tv.text('autostart-value'), 'On');
  assert.equal(tv.focus(), 'prysm-address');
});

/** Hold the portal's next `setTarget` until the returned function is called. */
function holdTarget(tv) {
  const { client } = tv.application;
  const send = client.setTarget;
  let release = () => {};
  client.setTarget = async (target) => {
    await new Promise((resolve) => {
      release = resolve;
    });
    client.setTarget = send;
    return send(target);
  };
  return () => release();
}

test('a lighting mode keeps focus and the track stays lit while it applies', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  const release = holdTarget(tv);
  tv.el('mode-screen_sync').focus();
  await tv.click('mode-screen_sync');
  // Waiting for the portal: nothing dims, nothing moves focus.
  for (const id of ['mode-screen_sync', 'mode-effects', 'mode-off'])
    assert(!tv.el(id).disabled, id);
  assert.equal(tv.el('mode-screen_sync').getAttribute('aria-busy'), 'true');
  assert.equal(tv.focus(), 'mode-screen_sync');
  await tv.click('mode-off');
  assert.deepEqual(
    tv.application.calls.filter((call) => call.startsWith('target:')),
    [],
    'a second press waits for the first',
  );
  await act(async () => release());
  await settle();
  assert.equal(tv.el('mode-screen_sync').getAttribute('aria-pressed'), 'true');
  assert.equal(tv.el('mode-screen_sync').getAttribute('aria-busy'), null);
  assert.equal(tv.focus(), 'mode-screen_sync');
});

test('full: saving Start with TV adds no button that would push the lights down', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  tv.el('autostart').focus();
  tv.supervisor.hold();
  await tv.click('autostart');
  assert(!tv.el('primary'), 'no "Please wait…" button meanwhile');
  assert.equal(tv.focus(), 'autostart');
  await act(async () => tv.supervisor.release());
  await settle();
  assert.equal(tv.text('autostart-value'), 'On');
  assert(!tv.el('primary'));
});

test('the title follows the portal, so it agrees with the lights card', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  assert.equal(tv.text('status-title'), 'Lights off');
  // Chosen on a phone: the portal knows before the TV service does.
  await act(async () => tv.application.client.setTarget('effects'));
  await settle();
  assert.equal(tv.supervisor.state().target, 'off', 'the service lags');
  assert.equal(tv.text('status-title'), 'WLED effects on');
});

test('the title and the lights card agree on what the lights do', async (t) => {
  // A portal failure was "Waiting for video" in the title; switching from
  // effects and turning off were read out as what the session still did.
  const tv = await mountTv(t, { flavor: 'full' });
  await act(async () =>
    tv.application.publish({
      target: 'screen_sync',
      session: {
        state: 'stopped',
        failure: { kind: 'adapter', message: 'WLED cannot be reached' },
      },
    }),
  );
  await settle();
  assert.equal(tv.text('status-title'), 'Screen sync stopped');
  assert.equal(
    tv.text('status-description'),
    'Try again, or choose another mode.',
  );
  assert.equal(tv.text('lighting-status'), 'Screen sync couldn’t start.');
  await act(async () =>
    tv.application.publish({
      target: 'screen_sync',
      session: { state: 'effects' },
    }),
  );
  await settle();
  assert.equal(tv.text('lighting-status'), 'Starting screen sync…');
  await act(async () =>
    tv.application.publish({
      target: 'off',
      session: { state: 'screen_sync' },
    }),
  );
  await settle();
  assert.equal(tv.text('lighting-status'), 'Lights are off.');
  assert.equal(tv.text('status-title'), 'Lights off');
});

test('both cards say effects start while the session still runs screen sync', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  await act(async () =>
    tv.application.publish({
      target: 'effects',
      session: { state: 'screen_sync' },
    }),
  );
  await settle();
  assert.equal(tv.text('status-title'), 'Starting WLED effects…');
  assert.equal(tv.text('lighting-status'), 'Starting WLED effects…');
});

test('the full status follows the portal: its failure, named by mode, and a faulted session', () => {
  const full = tvState('full', {
    target: 'screen_sync',
    worker: { phase: 'running' },
  });
  const draft = completePreferences({});
  const facts = (snapshot) => ({
    lights: true,
    address: null,
    lighting: lightingSituation(snapshot),
  });
  const failed = tvStatus(
    live(full),
    draft,
    facts({
      target: 'screen_sync',
      session: { state: 'stopped', failure: { kind: 'adapter', message: 'x' } },
    }),
  );
  assert.deepEqual(
    [failed.title, failed.description, failed.attention, failed.action],
    ['Screen sync stopped', 'See Your lights for what to do.', true, 'none'],
    'before the worker reports it, the service says nothing to do yet',
  );
  // As on a TV: the worker reports its session's failure, and the service
  // what is asked.
  const reported = (fix = 'try_again', change = {}) =>
    tvState('full', {
      target: 'screen_sync',
      worker: { phase: 'running' },
      problem: { asked: ask(fix), message: 'x' },
      ...change,
    });
  const sessionFailure = { kind: 'adapter', message: 'x' };
  const stopped = { state: 'stopped', failure: sessionFailure };
  const sync = (session) => facts({ target: 'screen_sync', session });
  assert.equal(
    tvStatus(live(reported()), draft, sync(stopped)).action,
    'retry',
  );
  // Settings Prysm can't use: the service says to change them.
  const unusable = tvStatus(
    live(reported('change_settings')),
    draft,
    sync({
      state: 'stopped',
      failure: { kind: 'invalid', field: 'output.address', message: 'x' },
    }),
  );
  assert.deepEqual(
    [unusable.description, unusable.action],
    ['Change the settings in the web portal.', 'none'],
  );
  // A start of screen sync from the effects that failed leaves them on.
  assert.equal(
    tvStatus(
      live(reported()),
      draft,
      sync({ state: 'effects', failure: sessionFailure }),
    ).title,
    'Screen sync didn’t start',
  );
  // What the service asks comes before the TV's power and video (it is
  // what needs the user); lights that are fine let them speak first.
  for (const [tv, service, session, title] of [
    [{ video: 'unavailable' }, reported(), stopped, 'Screen sync stopped'],
    [{ power: 'asleep' }, reported(), stopped, 'Screen sync stopped'],
    [
      { video: 'unavailable' },
      full,
      { state: 'screen_sync' },
      'Waiting for video',
    ],
    [{ power: 'asleep' }, full, { state: 'screen_sync' }, 'TV is asleep'],
  ])
    assert.equal(
      tvStatus(live({ ...service, ...tv }), draft, sync(session)).title,
      title,
      JSON.stringify([tv, session]),
    );
  // Before the portal answers, the lights that are off are still said off,
  // with what is asked.
  const offAsked = tvStatus(
    live(reported('try_again', { target: 'off' })),
    draft,
  );
  assert.deepEqual(
    [offAsked.title, offAsked.description],
    ['Lights off', 'Try again, or choose another mode.'],
  );
  assert.equal(tvStatus(live(reported()), draft).title, 'Screen sync stopped');
  // Also when the lights themselves failed (then not said to be still on).
  assert.equal(
    tvStatus(
      live(reported()),
      draft,
      sync({
        state: 'effects',
        failure: { kind: 'adapter', section: 'output', message: 'x' },
      }),
    ).title,
    'Screen sync didn’t start',
  );
  // The portal says what the lights do under what is asked: off with a
  // switch off that failed is off (tried again), effects with invalid
  // settings are on (their settings change in the portal).
  for (const [fix, target, session, title, action] of [
    ['try_again', 'off', stopped, 'Lights off', 'retry'],
    [
      'change_settings',
      'effects',
      { state: 'effects', failure: { kind: 'invalid', message: 'x' } },
      'WLED effects on',
      'none',
    ],
  ]) {
    const status = tvStatus(
      live(reported(fix, { target })),
      draft,
      facts({ target, session }),
    );
    assert.deepEqual([status.title, status.action], [title, action], target);
  }
  // Off with its failure and nothing asked: still off, its words said with
  // attention, and nothing to try again, also while the service still has
  // the mode.
  for (const [failure, description] of [
    [sessionFailure, 'The lights couldn’t be switched off.'],
    [
      { kind: 'invalid', field: 'output.address', message: 'x' },
      'Prysm can’t use these settings.',
    ],
  ])
    for (const service of [{ ...full, target: 'off' }, full]) {
      const status = tvStatus(
        live(service),
        draft,
        facts({ target: 'off', session: { state: 'stopped', failure } }),
      );
      assert.deepEqual(
        [status.title, status.description, status.attention, status.action],
        ['Lights off', description, true, 'none'],
        JSON.stringify([failure, service.target]),
      );
    }
  // Nothing to send for a target newer than the app.
  assert.equal(
    tvStatus(live(reported('try_again', { target: 'unknown' })), draft).action,
    'none',
  );
  // Effects that did not start are named as effects, not as screen sync.
  assert.equal(
    tvStatus(
      live(full),
      draft,
      facts({
        target: 'effects',
        session: {
          state: 'stopped',
          failure: { kind: 'adapter', message: 'x' },
        },
      }),
    ).title,
    'WLED effects stopped',
  );
  // Faulted, whatever the target: Prysm itself stopped, and the service
  // stops it next (a restart, or what it asks).
  for (const target of ['screen_sync', 'off']) {
    const faulted = tvStatus(
      live(full),
      draft,
      facts({
        target,
        session: {
          state: 'faulted',
          message: 'Lighting did not finish stopping in time.',
        },
      }),
    );
    assert.deepEqual(
      [faulted.title, faulted.description, faulted.attention],
      ['Prysm stopped responding', 'This takes a moment.', true],
      target,
    );
  }
  // Saved preferences that cannot be read wait for a faulted Prysm.
  const faultedUnreadable = tvStatus(
    live({ ...full, unreadable: 'x' }),
    draft,
    facts({ target: 'off', session: { state: 'faulted', message: 'x' } }),
  );
  assert.equal(faultedUnreadable.title, 'Prysm stopped responding');
  // Newer than this app: said, never read as off.
  for (const snapshot of [
    { target: 'unknown', session: { state: 'stopped' } },
    { target: 'screen_sync', session: { state: 'unknown' } },
  ]) {
    const newer = tvStatus(live(full), draft, facts(snapshot));
    assert.deepEqual(
      [newer.title, newer.description],
      [
        'Prysm is newer than this app',
        'Update the app to see what the lights do.',
      ],
      JSON.stringify(snapshot),
    );
  }
  assert.equal(
    tvStatus(live({ ...full, target: 'unknown' }), draft).title,
    'Prysm is newer than this app',
    'without the portal, the service’s target',
  );
  // A worker phase newer than this app: no start or stop.
  for (const flavor of ['full', 'slim']) {
    const updating = tvStatus(
      live({ ...tvState(flavor, {}), worker: { phase: 'unknown' } }),
      draft,
    );
    assert.deepEqual(
      [updating.title, updating.action, updating.label],
      ['Updating…', 'none', ''],
      flavor,
    );
  }
  const waiting = tvStatus(
    live(full),
    draft,
    facts({ target: 'screen_sync', session: { state: 'screen_sync' } }),
  );
  assert.equal(waiting.title, 'Waiting for video', 'the TV has no frames');
  // The portal says the TV refuses to share the video: said plainly, with
  // nothing to press, before the worker's own report.
  const refused = tvStatus(
    live(full),
    draft,
    facts({
      target: 'screen_sync',
      session: { state: 'screen_sync' },
      input: { fps: 0, picture: 'refused' },
    }),
  );
  assert.deepEqual(
    [refused.title, refused.description, refused.attention, refused.action],
    [
      'Video can’t be captured',
      'Screen sync continues when the video changes.',
      false,
      'none',
    ],
  );
  assert.equal(
    tvStatus(
      live(full),
      draft,
      facts({ target: 'effects', session: { state: 'effects' } }),
    ).title,
    'WLED effects on',
  );
});

test('the TV lights card says a failure with the lights off', async (t) => {
  // It was dropped while the target was off: nothing said why.
  for (const flavor of ['full', 'slim']) {
    const tv = await mountTv(t, {
      flavor,
      supervisor:
        flavor === 'slim'
          ? { preferences: { prysm_address: 'http://192.168.1.20:8787' } }
          : undefined,
    });
    await act(async () =>
      tv.application.publish({
        session: {
          state: 'stopped',
          failure: { kind: 'adapter', section: 'output', message: 'no answer' },
        },
      }),
    );
    await settle();
    assert.equal(tv.application.snapshot().target, 'off');
    assert.equal(tv.text('lighting-status'), 'The lights aren’t responding.');
    assert.match(
      document.querySelector('.card.lighting').className,
      /attention/,
    );
    if (flavor === 'full') assert.equal(tv.text('status-title'), 'Lights off');
    await tv.unmount();
  }
});

test('a refused press is said until the lights change', async (t) => {
  // It stayed, over the live state, until the next press.
  const tv = await mountTv(t, { flavor: 'full' });
  tv.application.failNext({ kind: 'unavailable', message: 'no answer' });
  await tv.click('mode-screen_sync');
  assert.equal(
    tv.text('lighting-status'),
    'A device isn’t available right now.',
  );
  // The command ran after all: the portal's state says so.
  await act(async () =>
    tv.application.publish({
      target: 'screen_sync',
      session: { state: 'screen_sync' },
    }),
  );
  await settle();
  assert.equal(tv.text('lighting-status'), 'Screen sync is running.');
  // Back as they were when it was refused: the old error stays gone.
  await act(async () =>
    tv.application.publish({ target: 'off', session: { state: 'stopped' } }),
  );
  await settle();
  assert.equal(tv.text('lighting-status'), 'Lights are off.');
});

test('a service newer than the app is shown, not offered setup', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: { prysm_address: 'http://192.168.1.20:8787' },
      state: { video: 'hdr' },
    },
  });
  assert.equal(tv.text('status-title'), 'Capture off');
  assert.equal(tv.text('primary'), 'Start capture');
});

test('a change asked for while another is sent follows it', async (t) => {
  // It was dropped without a word: only the first change was saved.
  const tv = await mountTv(t, {
    supervisor: { preferences: { prysm_address: 'http://192.168.1.20:8787' } },
  });
  tv.el('autostart').focus();
  tv.supervisor.hold();
  await tv.click('autostart');
  await tv.click('prysm-address');
  await tv.type('prysm-address-input', '192.168.1.30');
  await tv.submit('prysm-address-input');
  await act(async () => tv.supervisor.release());
  await settle();
  assert.deepEqual(
    tv.supervisor.calls
      .filter((call) => call.method === 'setPreferences')
      .map((call) => call.payload),
    [
      { autostart: true, prysm_address: 'http://192.168.1.20:8787' },
      { autostart: true, prysm_address: 'http://192.168.1.30:8787' },
    ],
  );
  assert.equal(tv.text('autostart-value'), 'On');
  assert.equal(tv.text('prysm-address-value'), '192.168.1.30:8787');
});

test('a failed setup stays said while the service is still missing', async (t) => {
  // Each failed resubscription, every 3 s, cleared it.
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const tv = await mountTv(t, { supervisor: { offline: true } });
  tv.supervisor.setSetupWorks(false);
  await tv.click('primary');
  const failed =
    'Setup didn’t finish. Service options › Diagnostics has the details.';
  assert.equal(tv.text('notice'), failed);
  await act(async () => t.mock.timers.tick(RESUBSCRIBE_MS));
  await settle();
  assert.equal(tv.text('status-title'), 'Prysm needs setting up');
  assert.equal(tv.text('notice'), failed);
});

test('only the newest address is checked into a warning', async (t) => {
  // An older address's slow check warned while the newer one was saved.
  const probes = [];
  const tv = await mountTv(t, {
    block: true,
    probe: (url) => new Promise((resolve) => probes.push({ url, resolve })),
  });
  const answer = (address, reachable) => {
    for (const probe of probes)
      if (probe.url.includes(address)) probe.resolve(reachable);
  };
  await tv.click('primary');
  await tv.type('prysm-address-input', '192.168.1.30');
  await tv.submit('prysm-address-input');
  await tv.click('prysm-address');
  await tv.type('prysm-address-input', '192.168.1.40');
  await tv.submit('prysm-address-input');
  await act(async () => answer('192.168.1.40', true));
  await settle();
  const refused =
    'Saved. Prysm answers there but doesn’t take commands from the TV; capture still works. The TV guide shows how to allow it.';
  assert.equal(tv.text('notice'), refused);
  await act(async () => answer('192.168.1.30', false));
  await settle();
  assert.equal(tv.text('notice'), refused, 'the older answer is ignored');
});

test('before the service answers, nothing flashes and focus waits', async (t) => {
  for (const flavor of ['full', 'slim']) {
    const tv = await mountTv(t, {
      flavor,
      hold: true,
      patterns: fakePlayer(),
      supervisor:
        flavor === 'slim'
          ? { preferences: { prysm_address: 'http://192.168.1.20:8787' } }
          : { state: { portal: null } },
    });
    assert.equal(tv.text('status-title'), 'Connecting…');
    assert.equal(tv.text('lighting-status'), 'Connecting to Prysm…', flavor);
    assert.equal(tv.text('calibrate-value'), 'Waiting for Prysm', flavor);
    assertFocused(document.body, `${flavor}: no focus placed yet`);
    // The web portal's place is kept, hidden: no address yet.
    assert(tv.el('portal-address').classList.contains('empty'), flavor);
    await act(async () => tv.supervisor.release());
    await settle();
    if (flavor === 'full') {
      // The full worker reports its address later; the place stays, hidden.
      assert(tv.el('portal-address').classList.contains('empty'));
      assert.equal(tv.text('portal-address-text'), '\u00a0');
      await act(async () =>
        tv.supervisor.publish({ portal: 'http://192.168.1.5:8787/' }),
      );
      await settle();
    }
    assert.equal(
      tv.text('portal-address-text'),
      flavor === 'full' ? '192.168.1.5:8787' : '192.168.1.20:8787',
    );
    assert(!tv.el('portal-address').classList.contains('empty'), flavor);
    assert.equal(tv.focus(), flavor === 'full' ? 'mode-off' : 'primary');
    await tv.unmount();
  }
});

test('the portal state stays shown for a few seconds while its stream is away', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const tv = await mountTv(t, { flavor: 'full' });
  tv.el('mode-off').focus();
  const said = tv.text('lighting-status');
  // Lost, and it does not come back.
  tv.application.client.observe = () =>
    Promise.reject(
      new ApplicationError({ kind: 'unavailable', message: 'gone' }),
    );
  await act(async () => tv.application.drop());
  await settle();
  assert.equal(tv.text('lighting-status'), said);
  assert(!tv.el('mode-off').disabled);
  assert.equal(tv.focus(), 'mode-off');
  await act(async () => t.mock.timers.tick(STALE_MS));
  await settle();
  assert.equal(
    tv.text('lighting-status'),
    'Can’t reach Prysm on this TV. Service options can repair it.',
  );
});

test('Up and Down scroll a long diagnostics text while focus stays on Back', async (t) => {
  const tv = await mountTv(t);
  tv.el('service-options').focus();
  await tv.click('service-options');
  await tv.click('diagnostics');
  assert.equal(tv.focus(), 'dialog-back');
  // jsdom has no layout: give the text a height and a scroll position.
  const text = tv.el('diagnostics-text');
  let top = 0;
  Object.defineProperty(text, 'clientHeight', { value: 300 });
  Object.defineProperty(text, 'scrollTop', {
    get: () => top,
    set: (value) => {
      top = value;
    },
  });
  await tv.key(DOWN);
  assert.equal(top, 200);
  await tv.key(UP);
  await tv.key(UP);
  assert.equal(top, -200, 'the browser clamps it at the top');
  assert.equal(tv.focus(), 'dialog-back');
});

test('removing the setup is tucked away, needs confirmation, and Back returns one level', async (t) => {
  const tv = await mountTv(t, {
    supervisor: { preferences: { prysm_address: 'http://192.168.1.20:8787' } },
  });
  tv.el('service-options').focus();
  await tv.click('service-options');
  await tv.click('cleanup');
  assert.equal(tv.text('dialog-title'), 'Remove service setup?');
  await tv.key(BACK);
  assert.equal(tv.text('dialog-title'), 'Service options');
  await tv.click('diagnostics');
  assert.match(
    tv.text('diagnostics-text'),
    /Prysm address: http:\/\/192\.168\.1\.20:8787/,
  );
  await tv.key(BACK);
  await tv.key(BACK);
  assert(!tv.el('modal'));
  assert.equal(tv.focus(), 'service-options');
  assert(!tv.methods().includes('uninstall'));
  await tv.click('service-options');
  await tv.click('cleanup');
  await tv.click('confirm-cleanup');
  await settle();
  assert(tv.methods().includes('uninstall'));
  assert.equal(tv.text('status-title'), 'Prysm needs setting up');
});

test('Back on the main screen asks before closing; Tab stays inside the dialog', async (t) => {
  const tv = await mountTv(t);
  await tv.key(BACK);
  assert.equal(tv.text('dialog-title'), 'Close Prysm?');
  assert.equal(tv.focus(), 'stay');
  await tv.key(TAB);
  assert.equal(tv.focus(), 'exit');
  await tv.key(TAB);
  assert.equal(tv.focus(), 'stay', 'two answers, no third Back button');
  await tv.key(RIGHT);
  assert.equal(tv.focus(), 'exit', 'side by side: Right moves between them');
  await tv.key(ESCAPE);
  assert(!tv.el('modal'));
  assert.equal(tv.exits(), 0);
  await tv.key(BACK);
  await tv.click('exit');
  assert.equal(tv.exits(), 1);
  assert(
    !tv.methods().includes('setEnabled'),
    'closing leaves capture as it is',
  );
});

test('hiding the page releases the subscriptions; showing it subscribes again', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
      },
    },
  });
  assert.equal(tv.supervisor.subscribers(), 1);
  assert(tv.application.calls.includes('observe'));
  Object.defineProperty(document, 'hidden', {
    value: true,
    configurable: true,
  });
  await act(async () =>
    document.dispatchEvent(new tv.dom.window.Event('visibilitychange')),
  );
  await settle();
  assert.equal(tv.supervisor.subscribers(), 0);
  assert(tv.application.calls.includes('unsubscribe'));
  Object.defineProperty(document, 'hidden', {
    value: false,
    configurable: true,
  });
  await act(async () =>
    document.dispatchEvent(new tv.dom.window.Event('visibilitychange')),
  );
  await settle();
  assert.equal(tv.supervisor.subscribers(), 1);
  assert.equal(
    tv.application.calls.filter((call) => call === 'observe').length,
    2,
  );
});

test('a page shown without a visibility event subscribes when it is focused', async (t) => {
  // The TV starts the page hidden and can show it with no event the app
  // sees; it stayed on "Connecting…" for ever.
  const tv = await mountTv(t);
  Object.defineProperty(document, 'hidden', {
    value: true,
    configurable: true,
  });
  await act(async () =>
    document.dispatchEvent(new tv.dom.window.Event('visibilitychange')),
  );
  await settle();
  assert.equal(tv.supervisor.subscribers(), 0);
  Object.defineProperty(document, 'hidden', {
    value: false,
    configurable: true,
  });
  await act(async () => window.dispatchEvent(new tv.dom.window.Event('focus')));
  await settle();
  assert.equal(tv.supervisor.subscribers(), 1);
});

test('one lost subscription keeps the last state; a second in a row offers setup', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
        autostart: true,
      },
    },
  });
  tv.el('autostart').focus();
  const portals = tv.application.calls.filter((c) => c === 'observe').length;
  await act(async () => tv.supervisor.drop('Service stopped'));
  await settle();
  // Nothing changes on screen, and OK there can never start a setup.
  assert.equal(tv.text('status-title'), 'Capture off');
  assert(!tv.el('autostart').disabled);
  assert.equal(tv.focus(), 'autostart');
  assert.equal(
    tv.application.calls.filter((c) => c === 'observe').length,
    portals,
    'the portal connection is kept',
  );
  await act(async () => t.mock.timers.tick(RESUBSCRIBE_MS));
  await settle();
  assert.equal(tv.text('status-title'), 'Capture off');
  assert.equal(tv.text('notice'), '', 'nothing was lost to report');
  // Lost again, and the next subscription fails too: now it says so.
  await act(async () => tv.supervisor.drop('Service stopped'));
  await settle();
  tv.supervisor.failNext('getState', 'Service is restarting');
  await act(async () => t.mock.timers.tick(RESUBSCRIBE_MS));
  await settle();
  assert.equal(tv.text('status-title'), 'Prysm needs setting up');
  assert.equal(tv.text('notice'), '');
  assert.equal(tv.text('autostart-value'), 'On', 'the settings stay shown');
  assert(tv.el('autostart').disabled, 'nothing to change without the service');
  await act(async () => t.mock.timers.tick(RESUBSCRIBE_MS));
  await settle();
  assert.equal(tv.text('status-title'), 'Capture off');
  assert.equal(tv.text('notice'), 'Service connected.');
});

test('a lost subscription then a missing service says so at once', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const tv = await mountTv(t, {
    supervisor: { preferences: { prysm_address: 'http://192.168.1.20:8787' } },
  });
  tv.supervisor.setOffline(true);
  await act(async () => tv.supervisor.drop('Unknown service name'));
  await settle();
  assert.equal(
    tv.text('status-title'),
    'Prysm needs setting up',
    'the bus says the service is not there',
  );
});

test('a waiting slim worker is described as waiting for Prysm, not as a failure', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
      },
      state: {
        enabled: true,
        worker: { phase: 'running' },
        uplink: {
          state: 'waiting',
          message:
            'waiting for the receiver: cannot connect to receiver: refused',
        },
      },
    },
  });
  assert.equal(tv.text('status-title'), 'Waiting for Prysm');
  assert.equal(
    tv.text('status-description'),
    'Check that Prysm is running on your computer.',
  );
  assert.doesNotMatch(
    tv.el('app').querySelector('.overview').className,
    /attention/,
  );
  assert.equal(tv.text('primary'), 'Stop capture');
});

test('slim lighting buttons drive the remote Prysm; leaving never turns it off', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
      },
    },
  });
  assert.equal(tv.text('lighting-status'), 'Lights are off.');
  await tv.click('mode-screen_sync');
  assert.equal(tv.application.snapshot().target, 'screen_sync');
  assert.equal(tv.text('lighting-status'), 'Screen sync is running.');
  assert(
    tv.el('lighting-status').querySelector('.visually-hidden'),
    'the chosen mode says it; the sentence is only read out',
  );
  assert.equal(tv.el('mode-screen_sync').getAttribute('aria-pressed'), 'true');
  assert.equal(tv.text('mode-effects'), 'WLED effects');
  tv.application.failNext({
    kind: 'adapter',
    message: 'WLED cannot be reached',
  });
  await tv.click('mode-effects');
  assert.equal(
    tv.text('lighting-status'),
    'WLED effects couldn’t start.',
    'in plain words, from the failure’s kind',
  );
  await tv.unmount();
  assert(tv.application.calls.includes('unsubscribe'));
  assert.equal(tv.application.snapshot().target, 'screen_sync');
  assert.equal(tv.supervisor.subscribers(), 0);
});

test('a Prysm that refuses the TV origin says how to allow it; an unreachable one says so', async (t) => {
  const refused = await mountTv(t, {
    block: true,
    probe: () => Promise.resolve(true),
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
      },
    },
  });
  await settle();
  assert.equal(
    refused.text('lighting-status'),
    'Prysm on your computer doesn’t take commands from the TV. The TV guide shows how to allow it.',
  );
  assert.doesNotMatch(document.body.textContent, /--allow-origin|null/);
  assert(
    refused.el('mode-screen_sync').disabled,
    'the modes stay in place, unavailable without a state',
  );
  await refused.unmount();
  const unreachable = await mountTv(t, {
    block: true,
    probe: () => Promise.resolve(false),
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
      },
    },
  });
  await settle();
  assert.equal(
    unreachable.text('lighting-status'),
    'Can’t reach Prysm on your computer. Check that it’s running.',
  );
  await unreachable.unmount();
  // The portal's own `forbidden` answer needs no probe, whatever its wording.
  let probes = 0;
  const forbidden = await mountTv(t, {
    block: { kind: 'forbidden', message: 'not from here' },
    probe: () => {
      probes++;
      return Promise.resolve(false);
    },
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
      },
    },
  });
  await settle();
  assert.match(
    forbidden.text('lighting-status'),
    /doesn’t take commands from the TV/,
  );
  assert.equal(probes, 0);
});

test('a remote Prysm without Remote capture is explained; an old browser gets the portal address', async (t) => {
  const tv = await mountTv(t, {
    portal: {
      capture: { kind: 'device', device: '/dev/video0' },
      output: wled,
    },
    supervisor: {
      preferences: {
        prysm_address: 'http://192.168.1.20:8787',
      },
    },
  });
  assert.equal(
    tv.text('lighting-status'),
    'This Prysm uses another picture. Choose LG TV on your network in its web portal.',
  );
  await tv.unmount();
  const old = await mountTv(t, {
    supported: false,
    supervisor: { preferences: { prysm_address: 'http://192.168.1.20:8787' } },
  });
  assert.match(old.text('lighting-status'), /too old/);
  assert.deepEqual(old.urls, [], 'no HTTP client is created');
});

test('full: the TV portal is used over loopback and its address is shown', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  assert.deepEqual(tv.urls, ['http://127.0.0.1:8787']);
  assert.equal(tv.text('portal-address-text'), '192.168.1.5:8787');
  assert.equal(tv.text('status-title'), 'Lights off');
  assert(!tv.el('prysm-address'), 'no slim rows');
  assert(!tv.el('capture'), 'capture is set in the portal only');
  assert(!tv.el('primary'), 'the lighting modes are the one control');
  // After a failure the supervisor does not retry, the button restarts it.
  // As on a TV: the engine's session failed, and the worker reports it.
  await act(async () => {
    tv.supervisor.publish({ target: 'screen_sync' });
    tv.application.publish({
      target: 'screen_sync',
      session: {
        state: 'stopped',
        failure: { kind: 'adapter', message: 'capture failed' },
      },
    });
  });
  await settle();
  assert.deepEqual(tv.supervisor.state().worker.problem, {
    asked: ask('try_again'),
    message: 'capture failed',
  });
  assert.equal(tv.text('status-title'), 'Screen sync stopped');
  assert.equal(tv.text('primary'), 'Try again');
  await tv.click('primary');
  await settle();
  // Off, then the target that failed.
  assert.deepEqual(
    tv.supervisor.calls
      .filter((call) => call.method === 'setTarget')
      .map((call) => call.payload.target),
    ['off', 'screen_sync'],
  );
  assert(!tv.methods().includes('setEnabled'), 'the slim package’s method');
});

test('full: while the service asks, the modes and Try again go to it', async (t) => {
  // No worker, so no portal: only the service can start Prysm again.
  for (const target of ['off', 'screen_sync']) {
    const tv = await mountTv(t, {
      flavor: 'full',
      supervisor: {
        state: { target, ...asking(UNANSWERED, 'switch off: no answer') },
      },
    });
    assert.equal(tv.text('status-title'), 'Can’t reach the lights', target);
    assert.equal(
      tv.text('status-description'),
      'Prysm stopped trying. Check them, then try again.',
      target,
    );
    assert.equal(
      tv.text('lighting-status'),
      'Choose a mode to start Prysm again.',
      target,
    );
    for (const mode of ['screen_sync', 'effects', 'off']) {
      assert(!tv.el(`mode-${mode}`).disabled, `${target}: ${mode} offered`);
      assert.equal(
        tv.el(`mode-${mode}`).getAttribute('aria-pressed'),
        String(mode === target),
        `${target}: ${mode}`,
      );
    }
    assert.deepEqual(tv.urls, [], 'no portal to call');
    // Try again: the target kept, once; it answers what was asked.
    await tv.click('primary');
    await settle();
    const targets = () =>
      tv.supervisor.calls
        .filter((call) => call.method === 'setTarget')
        .map((call) => call.payload.target);
    assert.deepEqual(targets(), [target], target);
    assert.equal(tv.supervisor.state().worker.phase, 'running', target);
    await tv.unmount();
  }
  // A mode chosen while asking goes to the service too.
  const tv = await mountTv(t, {
    flavor: 'full',
    supervisor: { state: asking(ask('restart_tv')) },
  });
  assert.equal(tv.text('status-title'), 'Restart the TV');
  assert(!tv.el('primary'), 'nothing to press that could help');
  await tv.click('mode-effects');
  await settle();
  assert.deepEqual(
    tv.supervisor.calls
      .filter((call) => call.method === 'setTarget')
      .map((call) => call.payload.target),
    ['effects'],
  );
  assert.equal(tv.supervisor.state().worker.phase, 'running');
  assert(tv.focus(), 'focus stays on a control');
});

test('a service set up again is answered: it starts its worker again', async (t) => {
  const setUp = (flavor) =>
    mountTv(t, {
      flavor,
      supervisor: {
        preferences:
          flavor === 'slim'
            ? { prysm_address: 'http://192.168.1.20:8787' }
            : {},
        state: {
          ...(flavor === 'slim'
            ? { enabled: true }
            : { target: 'screen_sync' }),
          ...asking(ask('set_up'), 'open capture: permission denied'),
        },
      },
    });
  for (const [flavor, method, payload] of [
    ['full', 'setTarget', { target: 'screen_sync' }],
    ['slim', 'setEnabled', { enabled: true }],
  ]) {
    const tv = await setUp(flavor);
    assert.equal(tv.text('status-title'), 'Prysm needs setting up', flavor);
    assert.equal(tv.text('primary'), 'Set up service', flavor);
    await tv.click('primary');
    await settle();
    assert(
      tv.supervisor.calls.some((call) => call.uri === HOMEBREW_EXEC),
      `${flavor}: set up`,
    );
    const answered = tv.supervisor.calls.filter(
      (call) => call.method === method,
    );
    assert.deepEqual(
      answered.map((call) => call.payload),
      [payload],
      flavor,
    );
    assert.equal(tv.supervisor.state().worker.phase, 'running', flavor);
    await tv.unmount();
  }
});

test('full: the lights card says why the lights do not follow the picture', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  await tv.click('mode-screen_sync');
  await act(async () =>
    tv.application.publish({
      session: { state: 'screen_sync' },
      input: { fps: 30, picture: 'arriving' },
      device: { issue: 'other_sender' },
    }),
  );
  assert.equal(
    tv.text('lighting-status'),
    'Another program is sending colours to the lights. Stop it to see screen sync alone.',
  );
  assert(!tv.el('lighting-status').querySelector('.visually-hidden'));
  assert.match(document.querySelector('.card.lighting').className, /attention/);
  await act(async () => tv.application.publish({ device: {} }));
  assert.equal(tv.text('lighting-status'), 'Screen sync is running.');
  assert(
    tv.el('lighting-status').querySelector('.visually-hidden'),
    'the chosen mode says it; read out only',
  );
  assert.doesNotMatch(
    document.querySelector('.card.lighting').className,
    /attention/,
  );
  await tv.unmount();
});

test('full: lights that don’t answer Off or their effects are said on both cards', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  for (const [target, detail] of [
    ['off', 'The lights switch off as soon as they answer.'],
    ['effects', 'WLED effects start as soon as the lights answer.'],
  ]) {
    await act(async () =>
      tv.application.publish({
        target,
        session: { state: 'stopped' },
        device: { issue: 'unreachable' },
      }),
    );
    await settle();
    assert.equal(
      tv.text('lighting-status'),
      `Can’t reach the lights. ${detail}`,
      target,
    );
    assert.match(
      document.querySelector('.card.lighting').className,
      /attention/,
      target,
    );
    assert.equal(tv.text('status-title'), 'Can’t reach the lights', target);
    assert.equal(tv.text('status-description'), detail, target);
  }
  // They answer: the effects show, and nothing calls for attention.
  await act(async () =>
    tv.application.publish({ session: { state: 'effects' }, device: {} }),
  );
  await settle();
  assert.equal(tv.text('status-title'), 'WLED effects on');
  assert.doesNotMatch(
    document.querySelector('.card.lighting').className,
    /attention/,
  );
  await tv.unmount();
});

test('neither package sets up capture, lights or the stream on the TV; both show the portal and the same labels', async (t) => {
  for (const flavor of ['full', 'slim']) {
    const tv = await mountTv(t, {
      flavor,
      supervisor: flavor === 'slim' ? { preferences: linked } : {},
    });
    const rows = [...document.querySelectorAll('.setting-list .setting')].map(
      (row) => row.id,
    );
    assert.deepEqual(
      rows,
      flavor === 'full'
        ? ['autostart', 'calibrate', 'service-options']
        : ['prysm-address', 'autostart', 'calibrate', 'service-options'],
    );
    assert.doesNotMatch(
      document.body.textContent,
      /fps|× 360|Video only|Other lights|LEDs/,
    );
    assert.match(tv.text('portal-address'), /on your phone/);
    assert.equal(
      tv.el('autostart').querySelector('.setting-label').textContent,
      'Start with TV',
    );
    assert.equal(
      document.querySelector('.brand-sub').textContent,
      'Ambient light',
    );
    assert.equal(
      document.querySelector('.section-label').textContent,
      flavor === 'full' ? 'Prysm on this TV' : 'Capture on this TV',
    );
    assert.equal(tv.text('lighting-title'), 'Your lights');
    assert.equal(
      tv.el('portal-address').querySelector('code').textContent,
      flavor === 'full' ? '192.168.1.5:8787' : shown(linked.prysm_address),
    );
    await tv.unmount();
  }
});

test('lights without a setup are named plainly with the portal address, in both packages', async (t) => {
  for (const flavor of ['full', 'slim']) {
    const tv = await mountTv(t, {
      flavor,
      supervisor: flavor === 'slim' ? { preferences: linked } : {},
      portal: { capture: portals[flavor].capture },
    });
    const address =
      flavor === 'full' ? '192.168.1.5:8787' : shown(linked.prysm_address);
    assert.equal(tv.text('status-title'), 'Set up your lights');
    assert.equal(
      tv.text('status-description'),
      'You add them from your phone, in the web portal.',
    );
    assert.equal(tv.text('portal-address-text'), address, 'shown beside it');
    assert.match(
      tv.el('status-title').closest('.overview').className,
      /attention/,
    );
    // The portal's QR code leads, in the modes' place; the modes, which
    // could not do anything yet, are not shown.
    assert(tv.el('portal-address').classList.contains('setup-lights'));
    assert.match(tv.text('portal-address'), /Scan this code with your phone/);
    assert(!document.querySelector('.card.lighting'), 'no modes yet');
    // Full has no primary button here: focus rests where OK only opens
    // something, never on the switch.
    assert.equal(tv.focus(), flavor === 'full' ? 'service-options' : 'primary');
    assert(!tv.el('mode-screen_sync'));
    // Once the portal has lights, the usual home returns by itself; nothing
    // starts until the lights are chosen.
    await act(async () =>
      tv.application.publish({
        settings: { ...tv.application.snapshot().settings, output: wled },
      }),
    );
    await settle();
    assert.notEqual(tv.text('status-title'), 'Set up your lights');
    assert(tv.el('mode-screen_sync'), 'the modes are back');
    assert(!tv.el('portal-address').classList.contains('setup-lights'));
    assert.equal(tv.application.snapshot().target, 'off');
    await tv.unmount();
  }
});

test('the lights setup hint gives way to failures and to a missing receiver', () => {
  const running = { worker: { phase: 'running' } };
  const linkedPrefs = { prysm_address: 'http://192.168.1.20:8787' };
  const fullOff = tvState('full', { ...running, target: 'off' });
  const noLights = { lights: false, address: 'http://192.168.1.20:8787' };
  const draft = completePreferences({});
  const status = (value) => tvStatus(live(value), draft, noLights);
  assert.equal(status(fullOff).title, 'Set up your lights');
  assert.equal(status(fullOff).action, 'none', 'screen sync needs lights');
  const slimOff = tvState('slim', { ...running, enabled: false }, linkedPrefs);
  assert.equal(
    tvStatus(live(slimOff), completePreferences(linkedPrefs), noLights).action,
    'enable',
    'capture does not',
  );
  // A running worker's portal has no lights: they come first. With no
  // worker there is no portal to say so, and what is asked is said.
  const lightsFirst = status(
    tvState('full', {
      ...running,
      target: 'screen_sync',
      problem: { asked: ask('try_again'), message: 'no output' },
    }),
  );
  assert.deepEqual(
    [lightsFirst.title, lightsFirst.action],
    ['Set up your lights', 'none'],
    'the button follows what the card says',
  );
  assert.equal(
    tvStatus(
      live(
        tvState('full', { target: 'screen_sync', ...asking(ask('try_again')) }),
      ),
      draft,
    ).title,
    'Screen sync stopped',
  );
  assert.equal(
    status(tvState('slim', { ...running, enabled: true })).title,
    'Set up this TV',
  );
  assert.equal(
    status(tvState('slim', { ...running, enabled: true }, linkedPrefs)).title,
    'Set up your lights',
  );
  assert.equal(
    tvStatus(live(fullOff), draft, { lights: true, address: null }).title,
    'Lights off',
  );
});

test('TV bundles are classic deferred scripts with relative URLs and no demo code', () => {
  for (const host of ['webos', 'webos-slim']) {
    const dir = resolve(process.cwd(), 'dist', host, 'tv-ui');
    const html = readFileSync(resolve(dir, 'index.html'), 'utf8');
    assert.match(html, /<script defer src="\.\/assets\/[^"]+\.js"><\/script>/);
    assert.match(html, /<link rel="stylesheet" href="\.\/assets\/[^"]+\.css">/);
    assert.doesNotMatch(html, /type="module"|<style|style=/);
    const assets = readdirSync(resolve(dir, 'assets'));
    assert.deepEqual(assets.map((name) => name.split('.').pop()).sort(), [
      'css',
      'js',
    ]);
    const script = readFileSync(
      resolve(
        dir,
        'assets',
        assets.find((name) => name.endsWith('.js')),
      ),
      'utf8',
    );
    assert.doesNotMatch(script, /fakeTvSupervisor|demo:|in-memory/);
    assert.doesNotMatch(
      script,
      /\?\.|\?\?|async function/,
      'Chromium 53 syntax',
    );
  }
});

/* ---- Slim stream encoding ---- */

const linked = {
  prysm_address: 'http://192.168.1.20:8787',
};

test('slim: the status card shows the encoding in use while streaming', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: linked,
      state: { enabled: true, worker: { phase: 'running' } },
    },
  });
  assert.equal(
    tv.text('status-meta'),
    '\u00a0',
    'nothing is in use before the receiver is connected',
  );
  await act(async () =>
    tv.supervisor.publish({
      capturing: true,
      fps: 30,
      encoding: 'h264_hardware',
    }),
  );
  await settle();
  assert.equal(tv.text('status-title'), 'Sending the picture');
  assert.equal(tv.text('status-meta'), '30 frames per second · Hardware H.264');
  await act(async () => tv.supervisor.publish({ encoding: 'raw' }));
  await settle();
  assert.equal(tv.text('status-meta'), '30 frames per second · Uncompressed');
  await act(async () =>
    tv.supervisor.publish({ capturing: false, fps: null, encoding: null }),
  );
  await settle();
  assert.equal(tv.text('status-meta'), '\u00a0');
});

test('slim: an encoder failure shows the error and sends the user to the portal shown below', async (t) => {
  const failure =
    'worker failed: cannot encode frame: hardware H.264 encoder cannot start: no device';
  const tv = await mountTv(t, {
    supervisor: {
      preferences: linked,
      state: {
        enabled: true,
        worker: { phase: 'running' },
        // Nothing is sent while the encoder failed.
        uplink: { state: 'encoder_failed', message: failure },
      },
    },
  });
  assert.equal(tv.text('status-title'), 'Encoding not supported');
  assert.equal(
    tv.text('status-description'),
    'Choose another encoding in Prysm’s web portal.',
  );
  assert.doesNotMatch(document.body.textContent, /no device/);
  assert.equal(
    tv.el('portal-address').querySelector('code').textContent,
    shown(linked.prysm_address),
  );
  assert(!tv.el('encoding'), 'no encoding setting on the TV');
});

test('slim: a receiver wait or another failure does not mention the encoding', async (t) => {
  const wait = await mountTv(t, {
    supervisor: {
      preferences: linked,
      state: {
        enabled: true,
        worker: { phase: 'running' },
        uplink: {
          state: 'waiting',
          message: 'waiting for the receiver: refused',
        },
      },
    },
  });
  assert.equal(wait.text('status-title'), 'Waiting for Prysm');
  assert.doesNotMatch(wait.text('status-description'), /encoding/);
  await wait.unmount();
  const tv = await mountTv(t, {
    supervisor: {
      preferences: linked,
      state: {
        enabled: true,
        ...asking(ask('try_again'), 'worker exited 1: capture failed'),
      },
    },
  });
  assert.equal(tv.text('status-title'), 'Capture stopped');
  assert.equal(
    tv.text('status-description'),
    'Stop and start capture again to retry.',
  );
  assert.equal(tv.text('status-meta'), '\u00a0', 'no countdown');
  await tv.key(DOWN);
  assert.equal(tv.focus(), 'mode-off', 'down from Stop capture: the lights');
});

test('full: no encoding setting, encoding in use or encoding switch', async (t) => {
  const tv = await mountTv(t, {
    flavor: 'full',
    supervisor: {
      state: {
        target: 'screen_sync',
        ...asking(ask('try_again'), 'encoder lost'),
      },
    },
  });
  assert.equal(tv.text('status-title'), 'Screen sync stopped');
  assert(!tv.el('encoding'));
  assert.doesNotMatch(document.body.textContent, /encoding/i);
});

/* ---- Calibration ---- */

/** A pattern player without video: records what the TV asks it to play. */
function fakePlayer(supported = true) {
  const player = {
    supported,
    plays: 0,
    stops: 0,
    video: null,
    pattern: null,
    failed: null,
    play(video, pattern, failed) {
      player.plays++;
      Object.assign(player, { video, pattern, failed });
      return () => player.stops++;
    },
  };
  return player;
}
const slimReady = {
  preferences: {
    prysm_address: 'http://192.168.1.20:8787',
  },
  state: { enabled: true },
};
/** What a phone does through the portal. */
async function phone(tv, calibration) {
  await act(async () => tv.application.client.setCalibration(calibration));
  await settle();
}
const visibleText = () => document.body.textContent.trim();

test('calibration needs MSE: without it the row says so and is disabled', async (t) => {
  // jsdom has no MediaSource, so the TV's own player is unsupported.
  for (const patterns of [undefined, fakePlayer(false)]) {
    const tv = await mountTv(t, { flavor: 'full', patterns });
    assert(tv.el('calibrate').disabled);
    assert.equal(tv.text('calibrate-value'), 'Needs a newer TV');
    await tv.unmount();
  }
  const old = await mountTv(t, {
    flavor: 'full',
    patterns: fakePlayer(),
    supported: false,
  });
  assert.equal(old.text('calibrate-value'), 'Needs a newer TV');
});

test('calibration is disabled with a reason while the lights are not driven from this TV', async (t) => {
  const cases = [
    [{}, 'Add the address first'],
    [
      { supervisor: { preferences: slimReady.preferences } },
      'Start capture first',
    ],
    [{ supervisor: slimReady, block: true }, 'Waiting for Prysm'],
    [
      {
        supervisor: slimReady,
        portal: {
          capture: { kind: 'device', device: '/dev/video0' },
          output: wled,
        },
      },
      'Uses another picture',
    ],
    [
      { flavor: 'full', portal: { capture: portals.full.capture } },
      'Set up lights first',
    ],
  ];
  for (const [options, reason] of cases) {
    const tv = await mountTv(t, { patterns: fakePlayer(), ...options });
    await settle();
    assert(tv.el('calibrate').disabled, reason);
    assert.equal(tv.text('calibrate-value'), reason);
    await tv.unmount();
  }
  const ready = await mountTv(t, {
    patterns: fakePlayer(),
    supervisor: slimReady,
  });
  assert(!ready.el('calibrate').disabled);
  assert.equal(ready.text('calibrate-value'), 'With your phone');
});

test('full: Calibrate turns screen sync on, shows the phone address and follows the patterns as video only', async (t) => {
  const player = fakePlayer();
  const tv = await mountTv(t, { flavor: 'full', patterns: player });
  await tv.click('calibrate');
  assert.deepEqual(
    tv.application.calls.filter((call) => call.startsWith('target:')),
    ['target:screen_sync'],
  );
  assert(!tv.el('app'), 'the main screen is closed');
  assert.equal(tv.text('calibration-address'), '192.168.1.5:8787/#/calibrate');
  assert.equal(tv.text('calibration-hint'), 'Scan with your phone to start.');
  assert(tv.el('calibration-qr'));
  // The gray pattern already plays under the waiting screen.
  assert.equal(player.plays, 1);
  assertSame(player.video, tv.el('pattern-video'), 'the player’s video');
  assert.equal(player.pattern(), 'gray_192');

  await phone(tv, { pattern: 'red', uncorrected: false });
  assert.equal(visibleText(), '', 'nothing but the video');
  assert(!document.querySelector('svg'));
  assert(tv.el('pattern-video'));
  assert.equal(player.pattern(), 'red');
  await phone(tv, { pattern: 'blue_pair', uncorrected: true });
  assert.equal(player.pattern(), 'blue_pair');
  assert.equal(player.plays, 1, 'one stream for every pattern');
  assert.equal(player.stops, 0);

  // The phone ends calibration: the TV waits for it again.
  await phone(tv, null);
  assert.equal(tv.text('calibration-address'), '192.168.1.5:8787/#/calibrate');
  assert.equal(player.pattern(), 'gray_192');
  assert.equal(player.plays, 1);

  // Back leaves: the pattern ends, lighting stays on.
  const before = tv.application.calls.length;
  await tv.key(BACK);
  assert.deepEqual(tv.application.calls.slice(before), ['calibration:end']);
  assert.equal(player.stops, 1);
  assert(tv.el('app'));
  assert.equal(tv.focus(), 'calibrate');
  assert(!tv.el('modal'), 'Back does not ask to close the app');
  assert(!tv.application.calls.includes('target:off'));
  assert.equal(tv.application.snapshot().target, 'screen_sync');
});

test('full: Calibrate does not open on a page hidden while screen sync starts', async (t) => {
  // The calibration screen opened on the hidden page, after hiding had
  // already ended calibration.
  const tv = await mountTv(t, { flavor: 'full', patterns: fakePlayer() });
  const hide = async (hidden) => {
    Object.defineProperty(document, 'hidden', {
      value: hidden,
      configurable: true,
    });
    await act(async () =>
      document.dispatchEvent(new tv.dom.window.Event('visibilitychange')),
    );
    await settle();
  };
  const release = holdTarget(tv);
  await tv.click('calibrate');
  try {
    await hide(true);
    await act(async () => release());
    await settle();
    assert(!tv.el('calibration-address'));
    assert(tv.el('app'));
  } finally {
    await hide(false);
  }
  assert(tv.el('app'), 'the main screen, shown again');
  assert.equal(tv.application.snapshot().target, 'screen_sync');
});

test('a pattern stays on screen while the portal stream comes back', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const player = fakePlayer();
  const tv = await mountTv(t, { flavor: 'full', patterns: player });
  await tv.click('calibrate');
  await phone(tv, { pattern: 'red', uncorrected: false });
  await act(async () => tv.application.drop());
  await settle();
  assert(!tv.el('calibration-address'), 'no QR code over it');
  assert.equal(player.pattern(), 'red');
  await act(async () => t.mock.timers.tick(250));
  await settle();
  assert(!tv.el('calibration-address'));
  assert.equal(player.pattern(), 'red');
});

test('slim: calibration uses the computer portal, already on, and ends when the app is hidden', async (t) => {
  const player = fakePlayer();
  const tv = await mountTv(t, { patterns: player, supervisor: slimReady });
  await act(async () => tv.application.client.setTarget('screen_sync'));
  await settle();
  const before = tv.application.calls.length;
  await tv.click('calibrate');
  assert(
    !tv.application.calls
      .slice(before)
      .some((call) => call.startsWith('target:')),
    'already on',
  );
  assert.equal(tv.text('calibration-address'), '192.168.1.20:8787/#/calibrate');
  await phone(tv, { pattern: 'coast', uncorrected: false });
  assert.equal(player.pattern(), 'coast');
  const callsBefore = tv.application.calls.length;
  Object.defineProperty(document, 'hidden', {
    value: true,
    configurable: true,
  });
  await act(async () =>
    document.dispatchEvent(new tv.dom.window.Event('visibilitychange')),
  );
  await settle();
  // Once each; which comes first is the browser's order of listeners.
  assert.deepEqual(tv.application.calls.slice(callsBefore).sort(), [
    'calibration:end',
    'unsubscribe',
  ]);
  assert.equal(player.stops, 1);
  assert(tv.el('app'));
  assert.equal(tv.application.snapshot().calibration, undefined);
  assert.equal(tv.application.snapshot().target, 'screen_sync');
  Object.defineProperty(document, 'hidden', {
    value: false,
    configurable: true,
  });
});

test('calibration stays closed when screen sync cannot start; a failed video says so', async (t) => {
  const player = fakePlayer();
  const tv = await mountTv(t, { flavor: 'full', patterns: player });
  tv.application.failNext({
    kind: 'adapter',
    message: 'Capture could not open',
  });
  await tv.click('calibrate');
  assert(tv.el('app'));
  assert.equal(tv.text('notice'), 'Screen sync couldn’t start.');
  assert.equal(player.plays, 0);

  await tv.click('calibrate');
  await phone(tv, { pattern: 'green', uncorrected: false });
  await act(async () => player.failed());
  await settle();
  assert.equal(
    tv.text('calibration-error'),
    'The pattern video stopped. Press Back and try again.',
  );
  assert(!tv.el('pattern-video'));
  await tv.key(BACK);
  assert(tv.el('app'));
});

test('the QR code encoder matches the reference encoder and refuses what does not fit', () => {
  const hash = (rows) =>
    createHash('sha256')
      .update(
        rows
          .map((row) => row.map((dark) => (dark ? '1' : '0')).join(''))
          .join('\n'),
      )
      .digest('hex');
  // From qrcode-generator 2.0.4 (the root TV app's), level M, with its mask;
  // pins the mask choice, which decoding cannot tell.
  assert.equal(
    hash(qrCode('http://192.168.1.5:8787/#/calibrate', 6)),
    '53a920e817486b59ffbf1c14a107334d4b472ebc1c0bd7c3c8bec25f22c1507a',
  );
  assert.equal(
    hash(qrCode('http://prysm-living-room.local:8787/#/calibrate', 2)),
    'b0a00bf61f1507d17d64bdf49a98ba07ca2ae4f1208efa91bc3ea8bd0fcace22',
  );
  assert.equal(qrCode('a').length, 21);
  assert.equal(qrCode('x'.repeat(106)).length, 41);
  assert.equal(qrCode('x'.repeat(107)), null);
});

test('an independent decoder reads every QR version and mask the encoder writes', () => {
  /** RGBA pixels of `modules`, 4 pixels per module inside a 4-module quiet zone. */
  function image(modules) {
    const scale = 4;
    const size = (modules.length + 8) * scale;
    const data = new Uint8ClampedArray(size * size * 4).fill(255);
    modules.forEach((row, y) =>
      row.forEach((dark, x) => {
        if (!dark) return;
        for (let dy = 0; dy < scale; dy++)
          for (let dx = 0; dx < scale; dx++) {
            const at = ((y + 4) * scale + dy) * size + (x + 4) * scale + dx;
            data.fill(0, at * 4, at * 4 + 3);
          }
      }),
    );
    return { data, size };
  }
  // The most bytes each of versions 1–6 holds at level M.
  for (const [version, bytes] of [14, 26, 42, 62, 84, 106].entries()) {
    const text = 'http://prysm.local:8787/#/calibrate?'
      .padEnd(bytes, 'x')
      .slice(0, bytes);
    for (let mask = 0; mask < 8; mask++) {
      const modules = qrCode(text, mask);
      assert.equal(modules.length, 21 + 4 * version, `version ${version + 1}`);
      const { data, size } = image(modules);
      const decoded = jsQR(data, size, size, {
        inversionAttempts: 'dontInvert',
      });
      assert.equal(decoded?.data, text, `version ${version + 1}, mask ${mask}`);
      assert.equal(decoded.version, version + 1);
    }
  }
});

test('the pattern fragments are MP4 clips, one per pattern of the contract', () => {
  const fragments = JSON.parse(
    readFileSync(
      resolve(process.cwd(), 'packages/tv-ui/pattern-fragments.json'),
      'utf8',
    ),
  );
  const openapi = JSON.parse(
    readFileSync(
      resolve(
        process.cwd(),
        'packages/contracts/generated/application.openapi.json',
      ),
      'utf8',
    ),
  );
  assert.deepEqual(
    Object.keys(fragments),
    openapi.components.schemas.Pattern.enum,
  );
  for (const [name, fragment] of Object.entries(fragments))
    assert.equal(
      Buffer.from(fragment, 'base64').subarray(4, 8).toString('latin1'),
      'ftyp',
      name,
    );
});

test('the pattern stream appends the requested pattern in sequence mode and stops on a stall', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const fragments = JSON.parse(
    readFileSync(
      resolve(process.cwd(), 'packages/tv-ui/pattern-fragments.json'),
      'utf8',
    ),
  );
  const bytes = (name) => Buffer.from(fragments[name], 'base64');
  class FakeBuffer extends EventTarget {
    mode = 'segments';
    updating = false;
    appended = [];
    range = null;
    get buffered() {
      const range = this.range;
      return {
        length: range ? 1 : 0,
        start: () => range[0],
        end: () => range[1],
      };
    }
    appendBuffer(data) {
      this.appended.push(Buffer.from(data));
      this.range = [this.range?.[0] ?? 0, (this.range?.[1] ?? 0) + 0.5];
    }
    remove(from, to) {
      this.range = [to, this.range[1]];
    }
  }
  const sources = [];
  class FakeSource extends EventTarget {
    static isTypeSupported = (type) => type === PATTERN_TYPE;
    constructor() {
      super();
      sources.push(this);
    }
    addSourceBuffer(type) {
      assert.equal(type, PATTERN_TYPE);
      return (this.buffer = new FakeBuffer());
    }
  }
  class FakeVideo extends EventTarget {
    currentTime = 0;
    src = '';
    paused = true;
    play() {
      this.paused = false;
      return Promise.resolve();
    }
    pause() {
      this.paused = true;
    }
    removeAttribute(name) {
      if (name === 'src') this.src = '';
    }
    load() {}
  }
  const revoked = [];
  const previous = globalThis.window;
  globalThis.window = {
    MediaSource: FakeSource,
    URL: {
      createObjectURL: () => 'blob:patterns',
      revokeObjectURL: (url) => revoked.push(url),
    },
  };
  t.after(() => {
    globalThis.window = previous;
  });
  assert.equal(msePatternPlayer.supported, true);

  const video = new FakeVideo();
  let pattern = 'gray_192';
  let failures = 0;
  streamPatterns(
    video,
    () => pattern,
    () => failures++,
  );
  assert.equal(video.src, 'blob:patterns');
  assert.equal(video.paused, false);
  const [source] = sources;
  source.dispatchEvent(new Event('sourceopen'));
  const { buffer } = source;
  assert.equal(buffer.mode, 'sequence');
  // Two seconds are buffered before playback starts, then no more.
  for (let i = 0; i < 8; i++) buffer.dispatchEvent(new Event('updateend'));
  assert.equal(buffer.appended.length, 5);
  for (const appended of buffer.appended)
    assert.deepEqual(appended, bytes('gray_192'));
  // Playing: a new pattern is appended when less than half a second is left.
  pattern = 'red';
  video.currentTime = 1.5;
  buffer.dispatchEvent(new Event('updateend'));
  assert.equal(buffer.appended.length, 5);
  // Playback moving appends too, without a finished append.
  video.currentTime = 2.2;
  video.dispatchEvent(new Event('timeupdate'));
  assert.equal(buffer.appended.length, 6);
  assert.deepEqual(buffer.appended.at(-1), bytes('red'));
  // Out of data: appended at once, without waiting for playback to move.
  video.currentTime = 2.6;
  buffer.range = [0, 2.6];
  video.dispatchEvent(new Event('waiting'));
  assert.equal(buffer.appended.length, 7);
  // Played video is dropped, keeping the last ten seconds.
  video.currentTime = 40;
  buffer.range = [0, 40.3];
  buffer.dispatchEvent(new Event('updateend'));
  assert.deepEqual(buffer.range, [30, 40.3]);
  // No progress for too long: the stream stops and reports it once.
  t.mock.timers.tick(STALL_MS + 200);
  assert.equal(failures, 1);
  assert.equal(video.src, '');
  assert.equal(video.paused, true);
  assert.deepEqual(revoked, ['blob:patterns']);
  t.mock.timers.tick(STALL_MS + 200);
  assert.equal(failures, 1);
});
