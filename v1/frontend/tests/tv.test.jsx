/**
 * The TV app on the real Luna client over an in-memory supervisor, and the
 * portal over the application fixture. Ported from the root TV app's
 * behaviour tests (crates/web/prysm-webos/tests/app.test.cjs) to v1 state.
 */
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { JSDOM } from 'jsdom';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { ApplicationError } from '@prysm/application-client';
import {
  CALL_TIMEOUT_MS,
  HOMEBREW_EXEC,
  RESUBSCRIBE_MS,
  SETUP_SCRIPT,
  isState,
  lunaTvService,
} from '@prysm/webos-platform';
import { TvApp } from '@prysm/tv-ui';
import {
  captureRate,
  derivedReceiver,
  diagnostics,
  encodingName,
  instanceAddress,
  listenPort,
  localPortal,
  receiverAddress,
  retrySeconds,
  tvStatus,
  completePreferences,
} from '../packages/tv-ui/model';
import { fakeApplicationPort } from '../testkit/application';
import { fakeTvSupervisor } from '../testkit/luna';
import {
  PATTERN_TYPE,
  STALL_MS,
  msePatternPlayer,
  streamPatterns,
} from '../packages/tv-ui/patterns';
import { qrCode } from '../packages/tv-ui/qr';
import { createHash } from 'node:crypto';
import jsQR from 'jsqr';

const fixtures = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/contracts/generated/fixtures.json'),
    'utf8',
  ),
);
const tick = () => new Promise((done) => setImmediate(done));
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
  slim: { capture: { kind: 'remote', listen: '0.0.0.0:7380' }, output: wled },
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
  const application = fakeApplicationPort(options.portal ?? portals[flavor]);
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
                        message: 'Cannot reach the portal.',
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
const state = (change = {}) => ({ ...fixtures['luna/State'][0], ...change });

test('generated Rust states are accepted; contract violations are not', () => {
  for (const value of fixtures['luna/State'])
    assert(isState(value), JSON.stringify(value));
  assert(!isState({ ...state(), flavor: 'lite' }));
  assert(!isState({ ...state(), worker: 'idle' }));
  assert(!isState({ ...state(), worker: 'running' }), 'the phase is an object');
  assert(
    !isState({ ...state(), worker: { phase: 'retrying' } }),
    'a restart says when',
  );
  assert(!isState({ ...state(), failure: 'x' }), 'a failure has a kind');
  assert(isState({ ...state(), failure: { kind: 'receiver', message: 'x' } }));
  assert(!isState({ ...state(), failure: { kind: 'later', message: 'x' } }));
  assert(!isState({ ...state(), fps: '30' }));
  assert(!isState({ ...state(), log: [1] }));
  assert(!isState(null));
});

test('the state subscription retries after unsubscribed, malformed and refused replies', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const bridge = manualBridge();
  const events = [];
  const stop = lunaTvService(bridge.factory).watch((event) =>
    events.push(event),
  );
  assert.equal(bridge.channels[0].uri, 'luna://io.prysm.tv.service/getState');
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
    ...state({ enabled: false }),
  });
  bridge.channels[3].reply({ returnValue: true, ...state({ enabled: true }) });
  assert.deepEqual(
    events.slice(-2).map((event) => event.online && event.state.enabled),
    [false, true],
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

  const enabling = service.setEnabled(true);
  t.mock.timers.tick(CALL_TIMEOUT_MS);
  await assert.rejects(enabling, { kind: 'timeout' });
  const channel = bridge.channels.at(-1);
  assert.equal(channel.cancelled, 1);
  // A retry works after the timeout.
  const again = service.setEnabled(true);
  bridge.channels
    .at(-1)
    .reply({ returnValue: true, ...state({ enabled: true }) });
  assert.equal((await again).enabled, true);
});

test('commands carry the contract payloads and fail with typed errors', async () => {
  const bridge = manualBridge();
  const service = lunaTvService(bridge.factory);
  const saving = service.setPreferences({ autostart: true });
  assert.equal(
    bridge.channels[0].uri,
    'luna://io.prysm.tv.service/setPreferences',
  );
  assert.deepEqual(bridge.channels[0].payload, { autostart: true });
  bridge.channels[0].reply({
    returnValue: false,
    errorText: 'only io.prysm.tv may change Prysm',
  });
  await assert.rejects(saving, {
    kind: 'refused',
    message: 'only io.prysm.tv may change Prysm',
  });
  const removing = service.uninstall();
  assert.deepEqual(bridge.channels[1].payload, {});
  bridge.channels[1].reply({ returnValue: true });
  assert.equal(await removing, undefined);
  const enabling = service.setEnabled(false);
  bridge.channels[2].reply({ returnValue: true, unexpected: true });
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
  assert.equal(
    SETUP_SCRIPT,
    '/media/developer/apps/usr/palm/services/io.prysm.tv.service/setup.sh',
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

test('addresses: instance origins, receivers and the derived receiver port', () => {
  assert.deepEqual(instanceAddress('192.168.1.20'), {
    value: 'http://192.168.1.20:8787',
  });
  assert.deepEqual(instanceAddress(' prysm.local/ '), {
    value: 'http://prysm.local:8787',
  });
  assert.deepEqual(instanceAddress('192.168.1.20:9000'), {
    value: 'http://192.168.1.20:9000',
  });
  assert.deepEqual(instanceAddress('HTTPS://prysm.home'), {
    value: 'https://prysm.home',
  });
  assert.deepEqual(instanceAddress('[fd00::2]'), {
    value: 'http://[fd00::2]:8787',
  });
  assert.deepEqual(instanceAddress(''), { value: null });
  assert('error' in instanceAddress('http://prysm.local/portal'));
  assert('error' in instanceAddress('ftp://prysm'));
  assert.deepEqual(receiverAddress('192.168.1.20:7380'), {
    value: '192.168.1.20:7380',
  });
  assert.deepEqual(
    receiverAddress(' prysm.local:7380 '),
    { value: 'prysm.local:7380' },
    'the TV resolves names',
  );
  assert.deepEqual(receiverAddress('[fd00::2]:7380'), {
    value: '[fd00::2]:7380',
  });
  assert.deepEqual(receiverAddress('Prysm-Desktop:7380'), {
    value: 'Prysm-Desktop:7380',
  });
  for (const bad of [
    '192.168.1.20',
    'prysm.local',
    'prysm.local:0',
    'prysm.local:70000',
    '0.0.0.0:7380',
    '[::]:7380',
    '192.168.1:7380',
    '300.1.1.1:7380',
    'prysm_tv:7380',
    '-prysm:7380',
    'prysm.local.:7380',
    'http://prysm.local:7380',
    'fd00::2:7380',
  ])
    assert('error' in receiverAddress(bad), bad);
  assert.equal(listenPort({ kind: 'remote', listen: '0.0.0.0:7380' }), 7380);
  assert.equal(listenPort({ kind: 'device', path: '/dev/video0' }), null);
  assert.equal(
    derivedReceiver('http://192.168.1.20:8787', 7380),
    '192.168.1.20:7380',
  );
  assert.equal(
    derivedReceiver('http://prysm.local:8787', 7380),
    'prysm.local:7380',
  );
  assert.equal(
    derivedReceiver('http://[fd00::2]:8787', 7380),
    '[fd00::2]:7380',
  );
  assert.equal(derivedReceiver('http://prysm.local:8787', null), null);
  assert.equal(
    localPortal('http://192.168.1.5:8787/'),
    'http://127.0.0.1:8787',
  );
  assert.equal(localPortal(null), null);
});

test('status card covers the supervisor states for both packages', () => {
  const slim = (change = {}, prefs = { receiver: '192.168.1.20:7380' }) => ({
    ...state(),
    flavor: 'slim',
    lighting: null,
    portal: null,
    preferences: prefs,
    enabled: true,
    worker: { phase: 'running' },
    power: 'active',
    video: 'available',
    capturing: false,
    failure: null,
    ...change,
  });
  const draft = (value = {}) =>
    completePreferences({ receiver: '192.168.1.20:7380', ...value });
  const title = (online, value, d = draft(), dirty = false) =>
    tvStatus(online, value, d, dirty).title;
  assert.equal(title(null, null), 'Connecting…');
  assert.equal(tvStatus(false, null, draft(), false).label, 'Set up service');
  assert.equal(title(true, slim({ capturing: true })), 'Sending the picture');
  assert(tvStatus(true, slim({ capturing: true }), draft(), false).ready);
  assert.equal(title(true, slim({ power: 'asleep' })), 'Waiting for TV');
  assert.equal(
    title(true, slim({ video: 'unavailable' })),
    'Waiting for video',
  );
  const waiting = tvStatus(
    true,
    slim({
      failure: {
        kind: 'receiver',
        message:
          'waiting for the receiver: cannot connect to receiver: refused',
      },
    }),
    draft(),
    false,
  );
  assert.equal(waiting.title, 'Waiting for Prysm');
  assert.equal(waiting.attention, false, 'waiting is not a failure');
  const retrying = tvStatus(
    true,
    slim({
      worker: { phase: 'retrying', seconds: 4 },
      failure: { kind: 'other', message: 'worker exited 1' },
    }),
    draft(),
    false,
  );
  assert.equal(retrying.title, 'Capture needs attention');
  assert.match(retrying.description, /worker exited 1\. Retrying in 4 s\./);
  assert.equal(
    title(
      true,
      slim({ failure: { kind: 'other', message: 'capture failed' } }),
    ),
    'Capture needs attention',
  );
  assert.equal(
    title(true, slim({ worker: { phase: 'stopping' } })),
    'Stopping capture…',
  );
  const notSet = tvStatus(
    true,
    slim({ enabled: false }, {}),
    completePreferences({}),
    false,
  );
  assert.deepEqual(
    [notSet.title, notSet.label, notSet.action],
    ['Not set up', 'Set Prysm address', 'address'],
  );
  assert.equal(
    tvStatus(true, slim({ enabled: false }), draft({ autostart: true }), true)
      .label,
    'Save and start capture',
  );
  assert.equal(
    tvStatus(true, slim({ enabled: false }), draft(), false).label,
    'Start capture',
  );
  assert.equal(tvStatus(true, slim(), draft(), false).label, 'Stop capture');
  assert.equal(title(true, slim({ enabled: false })), 'Capture off');
  const full = (change) => ({
    ...state(),
    flavor: 'full',
    worker: { phase: 'running' },
    power: 'active',
    video: 'available',
    failure: null,
    ...change,
  });
  assert.deepEqual(
    [
      tvStatus(true, full({ enabled: false, lighting: 'off' }), draft(), false),
    ].map((s) => [s.title, s.label]),
    [['Lights off', 'Start screen sync']],
  );
  assert.equal(
    title(
      true,
      full({ enabled: true, lighting: 'processed', capturing: true }),
    ),
    'Screen sync is on',
  );
  assert.equal(
    title(true, full({ enabled: true, lighting: 'device_native' })),
    'WLED effects on',
  );
  const noFrames = tvStatus(
    true,
    full({ enabled: true, lighting: 'processed', capturing: false }),
    draft(),
    false,
  );
  assert.equal(
    noFrames.title,
    'Waiting for video',
    'lighting on without frames',
  );
  assert.match(noFrames.description, /own WLED effects/);
  assert.equal(noFrames.attention, false);
  assert.equal(
    title(
      true,
      full({
        enabled: true,
        lighting: 'processed',
        capturing: false,
        portal: null,
      }),
    ),
    'Starting Prysm…',
  );
  assert.match(
    tvStatus(
      true,
      full({ enabled: true, video: 'unavailable' }),
      draft(),
      false,
    ).description,
    /own WLED effects/,
  );
  assert.equal(
    tvStatus(true, full({ enabled: true }), draft(), false).label,
    'Turn lights off',
  );
});

test('slim encoder failures point to the portal; waits, other failures and the full package do not', () => {
  const slim = (change = {}) => ({
    ...state(),
    flavor: 'slim',
    lighting: null,
    portal: null,
    preferences: { receiver: '192.168.1.20:7380' },
    enabled: true,
    worker: { phase: 'running' },
    power: 'active',
    video: 'available',
    capturing: false,
    failure: null,
    encoding: null,
    ...change,
  });
  const draft = completePreferences({ receiver: '192.168.1.20:7380' });
  const status = (value) => tvStatus(true, value, draft, false);
  const hint =
    'Choose another stream encoding under Capture in Prysm’s web portal.';
  assert.deepEqual(
    ['h264_hardware', 'h264_software', 'raw'].map(encodingName),
    ['Hardware H.264', 'Software H.264', 'Uncompressed'],
  );
  assert.equal(
    'changeEncoding' in status(slim()),
    false,
    'the TV offers no encoding switch',
  );
  const retrying = status(
    slim({
      worker: { phase: 'retrying', seconds: 4 },
      failure: {
        kind: 'encoder',
        message: 'worker failed: hardware H.264 encoder cannot start',
      },
    }),
  );
  assert.equal(retrying.title, 'Capture needs attention');
  assert.equal(
    retrying.description,
    `worker failed: hardware H.264 encoder cannot start. Retrying in 4 s. ${hint}`,
  );
  const failed = status(
    slim({
      failure: { kind: 'encoder', message: 'worker failed: encoder lost' },
    }),
  );
  assert.equal(
    failed.description,
    `worker failed: encoder lost. ${hint} Open Service options for diagnostics.`,
  );
  assert(failed.attention);
  const other = status(
    slim({
      failure: {
        kind: 'other',
        message: 'worker failed: the encoder of capture lost',
      },
    }),
  );
  assert.equal(other.title, 'Capture needs attention');
  assert.equal(
    other.description,
    'worker failed: the encoder of capture lost. Open Service options for diagnostics.',
    'only the structured kind counts, not the text',
  );
  const otherRetrying = status(
    slim({
      worker: { phase: 'retrying', seconds: 4 },
      failure: { kind: 'other', message: 'worker exited 1' },
    }),
  );
  assert.doesNotMatch(otherRetrying.description, /encoding/);
  const paused = status(slim({ paused: true, encoding: 'h264_hardware' }));
  assert.deepEqual(
    [paused.title, paused.attention, paused.ready],
    ['Paused by Prysm', false, false],
  );
  assert.match(paused.description, /lights are off on your computer/);
  assert.match(
    diagnostics(slim({ paused: true })),
    /Capturing: no, paused by Prysm \(lights off on the computer\)/,
  );
  assert.match(diagnostics(slim()), /Capturing: no\n/);
  const wait = status(
    slim({
      failure: {
        kind: 'receiver',
        message: 'waiting for the receiver: refused',
      },
    }),
  );
  assert.deepEqual([wait.title, wait.attention], ['Waiting for Prysm', false]);
  assert.doesNotMatch(wait.description, /encoding/);
  const unknownWait = status(
    slim({
      failure: { kind: 'other', message: 'waiting for the receiver: refused' },
    }),
  );
  assert.equal(
    unknownWait.title,
    'Capture needs attention',
    'the text prefix no longer decides',
  );
  const full = {
    ...state(),
    flavor: 'full',
    worker: { phase: 'retrying', seconds: 4 },
    power: 'active',
    video: 'available',
    failure: { kind: 'encoder', message: 'worker exited 1' },
  };
  assert.doesNotMatch(status(full).description, /encoding/);
  assert.match(
    diagnostics(slim({ encoding: 'raw' })),
    /Stream encoding: Uncompressed in use/,
  );
  assert.match(diagnostics(slim()), /Stream encoding: none in use/);
});

test('a pending restart counts down from the published seconds', () => {
  const phase = { phase: 'retrying', seconds: 4 };
  assert.deepEqual(
    [0, 999, 1000, 3500, 4000, 9000].map((ms) => retrySeconds(phase, ms)),
    [4, 4, 3, 1, 0, 0],
  );
  assert.equal(retrySeconds({ phase: 'running' }, 0), null);
  const value = {
    ...state(),
    worker: phase,
    failure: { kind: 'other', message: 'worker failed' },
  };
  const draft = completePreferences({});
  assert.equal(
    tvStatus(true, value, draft, false, null, 1500).description,
    'worker failed. Retrying in 3 s.',
  );
  assert.equal(
    tvStatus(true, value, draft, false, null, 4000).description,
    'worker failed. Retrying now…',
  );
});

test('the TV counts a restart down by itself', async (t) => {
  t.mock.timers.enable({ apis: ['setInterval', 'Date'], now: 0 });
  const tv = await mountTv(t, {
    supervisor: {
      preferences: { receiver: '192.168.1.20:7380' },
      state: {
        enabled: true,
        worker: { phase: 'retrying', seconds: 4 },
        failure: { kind: 'other', message: 'worker exited 1' },
      },
    },
  });
  assert.equal(
    tv.text('status-description'),
    'worker exited 1. Retrying in 4 s.',
  );
  await act(async () => t.mock.timers.tick(2_000));
  assert.equal(
    tv.text('status-description'),
    'worker exited 1. Retrying in 2 s.',
  );
});

test('the capture rate is shown while the worker measures it', async (t) => {
  assert.equal(captureRate(state({ fps: 30 })), '30 frames/s');
  assert.equal(captureRate(state({ fps: 0 })), 'waiting for frames');
  assert.equal(captureRate(state({ fps: null })), null);
  assert.match(diagnostics(state({ fps: 30 })), /Capture rate: 30 frames\/s/);
  assert.match(diagnostics(state({ fps: null })), /Capture rate: not measured/);
  const tv = await mountTv(t, {
    supervisor: {
      preferences: { receiver: '192.168.1.20:7380' },
      state: {
        enabled: true,
        worker: { phase: 'running' },
        capturing: true,
        fps: 30,
      },
    },
  });
  assert.equal(tv.text('capture-rate'), 'Capture rate: 30 frames/s');
  await act(async () => tv.supervisor.publish({ capturing: false, fps: null }));
  await settle();
  assert.equal(tv.el('capture-rate'), null);
});

test('preferences are completed with their defaults', () => {
  assert.deepEqual(completePreferences({ autostart: true }), {
    autostart: true,
    instance: null,
    receiver: null,
  });
});

/* ---- Screens ---- */

test('an unavailable service offers setup, keeps the error visible and connects after setup', async (t) => {
  const tv = await mountTv(t, { supervisor: { offline: true } });
  assert.equal(tv.text('status-title'), 'TV service unavailable');
  assert.equal(tv.text('primary'), 'Set up service');
  assert.match(tv.text('notice'), /Unknown service name/);
  assert.equal(tv.focus(), 'primary');
  await tv.click('primary');
  await settle();
  const exec = tv.supervisor.calls.find((call) =>
    call.uri.startsWith('luna://org.webosbrew'),
  );
  assert.deepEqual(exec.payload, { command: SETUP_SCRIPT });
  assert.equal(tv.text('status-title'), 'Not set up');
});

test('remote arrows move focus through rows and columns without changing settings', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
      },
    },
  });
  assert.equal(tv.focus(), 'primary');
  await tv.key(DOWN);
  assert.equal(tv.focus(), 'service-options');
  await tv.key(DOWN);
  assert.equal(
    tv.focus(),
    'mode-off',
    'the lighting row is one stop, entered at its chosen mode',
  );
  await tv.key(LEFT);
  assert.equal(tv.focus(), 'mode-device_native');
  await tv.key(RIGHT);
  await tv.key(RIGHT);
  assert.equal(
    tv.focus(),
    'instance',
    'the row edge continues to the settings column',
  );
  await tv.key(DOWN);
  await tv.key(DOWN);
  assert.equal(tv.focus(), 'autostart');
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
  assert.equal(tv.focus(), 'instance');
  assert(!tv.methods().includes('setPreferences'));
  assert.equal(tv.text('saved-state'), 'Saved');
});

test('OK opens choices, arrows only focus, Back cancels, and Save alone persists', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
      },
    },
  });
  tv.el('autostart').focus();
  await tv.click('autostart');
  assert.equal(tv.text('dialog-title'), 'Start with TV');
  assert.equal(
    document.activeElement.getAttribute('aria-pressed'),
    'true',
    'the current choice is focused',
  );
  await tv.key(DOWN);
  await tv.key(BACK);
  assert.equal(tv.el('modal'), null);
  assert.equal(tv.focus(), 'autostart', 'focus returns to the row');
  assert.equal(tv.text('autostart-value'), 'Off');
  await tv.click('autostart');
  await tv.choose('On');
  assert.equal(tv.text('autostart-value'), 'On');
  assert.equal(tv.text('saved-state'), 'Unsaved changes');
  assert(!tv.methods().includes('setPreferences'), 'choosing does not save');
  await tv.click('save');
  const saved = tv.supervisor.calls.find(
    (call) => call.method === 'setPreferences',
  ).payload;
  assert.equal(saved.autostart, true);
  assert.equal(saved.instance, 'http://192.168.1.20:8787');
  assert.equal(tv.text('saved-state'), 'Saved');
  assert.equal(tv.supervisor.state().preferences.autostart, true);
});

test('the Prysm address is explicit: invalid text is explained and Back keeps the old one', async (t) => {
  const tv = await mountTv(t);
  assert.equal(tv.text('primary'), 'Set Prysm address');
  await tv.click('primary');
  assert.equal(tv.text('dialog-title'), 'Prysm address');
  assert.equal(tv.focus(), 'instance-input');
  await tv.key(LEFT);
  assert.equal(tv.focus(), 'instance-input', 'left and right move the caret');
  await tv.type('instance-input', 'http://prysm.local/x');
  await tv.submit('instance-input');
  assert.match(tv.text('instance-input-error'), /Enter an address/);
  await tv.type('instance-input', ' 192.168.1.30 ');
  await tv.key(BACK);
  assert.equal(tv.text('instance-value'), 'Add address');
  await tv.click('instance');
  assert.equal(tv.el('instance-input').value, '');
  await tv.type('instance-input', '192.168.1.30');
  await tv.submit('instance-input');
  assert.equal(tv.el('modal'), null);
  assert.equal(tv.text('instance-value'), 'http://192.168.1.30:8787');
  assert.equal(tv.text('saved-state'), 'Unsaved changes');
});

test('saving a Prysm address checks that a portal answers there, and only warns', async (t) => {
  const save = async (options) => {
    const tv = await mountTv(t, options);
    await tv.click('primary');
    await tv.type('instance-input', '192.168.1.30');
    await tv.submit('instance-input');
    await settle();
    await tv.click('save');
    await settle();
    assert.equal(
      tv.supervisor.state().preferences.instance,
      'http://192.168.1.30:8787',
      'saved anyway',
    );
    const notice = tv.text('notice');
    await tv.unmount();
    return notice;
  };
  assert.equal(
    await save({ block: true }),
    'Saved. No Prysm answers at http://192.168.1.30:8787. Check that Prysm is running and that the address is right.',
  );
  const refused =
    'Saved. http://192.168.1.30:8787 answers, but Prysm there does not allow control from the TV: start it with --allow-origin null. Capture works without it.';
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
  assert.equal(await save({}), 'Settings saved.', 'a portal answers');
});

test('the receiver is derived from the remote capture port, then Save and start capture sends it first', async (t) => {
  const tv = await mountTv(t);
  await tv.click('primary');
  await tv.type('instance-input', '192.168.1.20');
  await tv.submit('instance-input');
  await settle();
  assert.deepEqual(tv.urls, ['http://192.168.1.20:8787']);
  assert.equal(tv.text('receiver-value'), '192.168.1.20:7380');
  assert.match(tv.text('notice'), /Frames will go to 192\.168\.1\.20:7380/);
  assert.equal(tv.text('primary'), 'Save and start capture');
  await tv.click('primary');
  await settle();
  assert.deepEqual(
    tv.methods().filter((m) => m !== 'getState'),
    ['setPreferences', 'setEnabled'],
  );
  assert.deepEqual(
    [
      tv.supervisor.state().preferences.instance,
      tv.supervisor.state().preferences.receiver,
    ],
    ['http://192.168.1.20:8787', '192.168.1.20:7380'],
  );
  assert.equal(tv.text('primary'), 'Stop capture');
});

test('a Prysm addressed by name gets a receiver by name, and the receiver form accepts names', async (t) => {
  const tv = await mountTv(t);
  await tv.click('primary');
  await tv.type('instance-input', 'prysm.local');
  await tv.submit('instance-input');
  await settle();
  assert.deepEqual(tv.urls, ['http://prysm.local:8787']);
  assert.equal(tv.text('receiver-value'), 'prysm.local:7380');
  await tv.click('receiver');
  await tv.type('receiver-input', '0.0.0.0:7380');
  await tv.submit('receiver-input');
  assert.match(
    tv.text('receiver-input-error'),
    /prysm\.local:7380 or 192\.168\.1\.20:7380/,
  );
  await tv.type('receiver-input', 'lights.home.arpa:7380');
  await tv.submit('receiver-input');
  assert.equal(tv.el('modal'), null);
  assert.equal(tv.text('receiver-value'), 'lights.home.arpa:7380');
});

test('a failed save keeps the draft and never enables', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
      },
    },
  });
  tv.supervisor.failNext(
    'setPreferences',
    'invalid setPreferences request: receiver `x`: needs a fixed port',
  );
  await tv.click('autostart');
  await tv.choose('On');
  await tv.click('primary');
  assert.equal(
    tv.text('notice'),
    'invalid setPreferences request: receiver `x`: needs a fixed port',
  );
  assert(!tv.methods().includes('setEnabled'));
  assert.equal(tv.text('autostart-value'), 'On');
  assert.equal(tv.text('saved-state'), 'Unsaved changes');
});

test('status updates keep unsaved choices and focus; a change made elsewhere replaces the edit', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
      },
    },
  });
  await tv.click('autostart');
  await tv.choose('On');
  tv.el('receiver').focus();
  await act(async () =>
    tv.supervisor.publish({
      capturing: true,
      enabled: true,
      worker: { phase: 'running' },
    }),
  );
  await settle();
  assert.equal(tv.text('autostart-value'), 'On');
  assert.equal(tv.focus(), 'receiver');
  const prefs = tv.supervisor.state().preferences;
  await act(async () =>
    tv.supervisor.publish({ preferences: { ...prefs, autostart: true } }),
  );
  await settle();
  assert.equal(tv.text('autostart-value'), 'On');
  assert.match(tv.text('notice'), /changed elsewhere/);
  assert.equal(tv.text('saved-state'), 'Saved');
});

test('removing the setup is tucked away, needs confirmation, and Back returns one level', async (t) => {
  const tv = await mountTv(t, {
    supervisor: { preferences: { receiver: '192.168.1.20:7380' } },
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
    /Frames go to: 192\.168\.1\.20:7380/,
  );
  await tv.key(BACK);
  await tv.key(BACK);
  assert.equal(tv.el('modal'), null);
  assert.equal(tv.focus(), 'service-options');
  assert(!tv.methods().includes('uninstall'));
  await tv.click('service-options');
  await tv.click('cleanup');
  await tv.click('confirm-cleanup');
  await settle();
  assert(tv.methods().includes('uninstall'));
  assert.equal(tv.text('status-title'), 'TV service unavailable');
});

test('Back on the main screen asks before closing; Tab stays inside the dialog', async (t) => {
  const tv = await mountTv(t);
  await tv.key(BACK);
  assert.equal(tv.text('dialog-title'), 'Close Prysm?');
  assert.equal(tv.focus(), 'stay');
  await tv.key(TAB);
  assert.equal(tv.focus(), 'exit');
  await tv.key(TAB);
  assert.equal(tv.focus(), 'dialog-back');
  await tv.key(TAB);
  assert.equal(tv.focus(), 'stay');
  await tv.key(ESCAPE);
  assert.equal(tv.el('modal'), null);
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
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
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

test('a lost subscription recovers after 3 s without losing the draft', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
      },
    },
  });
  await tv.click('autostart');
  await tv.choose('On');
  assert.equal(tv.text('autostart-value'), 'On');
  await act(async () => tv.supervisor.drop('Service stopped'));
  await settle();
  assert.equal(tv.text('status-title'), 'TV service unavailable');
  assert.equal(tv.text('notice'), 'Service stopped');
  assert.equal(tv.text('autostart-value'), 'On');
  await act(async () => t.mock.timers.tick(RESUBSCRIBE_MS));
  await settle();
  assert.equal(tv.text('status-title'), 'Capture off');
  assert.equal(tv.text('notice'), 'Service connected.');
  assert.equal(tv.text('autostart-value'), 'On');
  assert.equal(tv.text('saved-state'), 'Unsaved changes');
});

test('a waiting slim worker is described as waiting for Prysm, not as a failure', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
      },
      state: {
        enabled: true,
        worker: { phase: 'running' },
        failure: {
          kind: 'receiver',
          message:
            'waiting for the receiver: cannot connect to receiver: refused',
        },
      },
    },
  });
  assert.equal(tv.text('status-title'), 'Waiting for Prysm');
  assert.match(tv.text('status-description'), /192\.168\.1\.20:7380/);
  assert.equal(tv.text('primary'), 'Stop capture');
});

test('slim lighting buttons drive the remote Prysm; leaving never turns it off', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: {
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
      },
    },
  });
  assert.equal(tv.text('lighting-status'), 'Lights are off.');
  await tv.click('mode-processed');
  assert.equal(tv.application.snapshot().target, 'processed');
  assert.equal(tv.text('lighting-status'), 'Screen sync is running.');
  assert.match(
    tv.el('lighting-status').className,
    /visually-hidden/,
    'the chosen mode says it; the sentence is only read out',
  );
  assert.equal(tv.el('mode-processed').getAttribute('aria-pressed'), 'true');
  assert.equal(tv.text('mode-device_native'), 'WLED effects');
  tv.application.failNext({
    kind: 'adapter',
    message: 'WLED cannot be reached',
  });
  await tv.click('mode-device_native');
  assert.match(
    tv.text('lighting-title') + document.querySelector('.lighting').textContent,
    /WLED cannot be reached/,
  );
  await tv.unmount();
  assert(tv.application.calls.includes('unsubscribe'));
  assert.equal(tv.application.snapshot().target, 'processed');
  assert.equal(tv.supervisor.subscribers(), 0);
});

test('a Prysm that refuses the TV origin says how to allow it; an unreachable one says so', async (t) => {
  const refused = await mountTv(t, {
    block: true,
    probe: () => Promise.resolve(true),
    supervisor: {
      preferences: {
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
      },
    },
  });
  await settle();
  assert.match(
    refused.text('lighting-status'),
    /does not allow control from the TV/,
  );
  assert.match(refused.text('lighting-detail'), /--allow-origin null/);
  assert(
    refused.el('mode-processed').closest('[hidden]'),
    'no modes without a state',
  );
  await refused.unmount();
  const unreachable = await mountTv(t, {
    block: true,
    probe: () => Promise.resolve(false),
    supervisor: {
      preferences: {
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
      },
    },
  });
  await settle();
  assert.match(
    unreachable.text('lighting-status'),
    /Cannot reach Prysm at http:\/\/192\.168\.1\.20:8787/,
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
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
      },
    },
  });
  await settle();
  assert.match(
    forbidden.text('lighting-status'),
    /does not allow control from the TV/,
  );
  assert.equal(probes, 0);
});

test('a remote Prysm without Remote capture is explained; an old browser gets the portal address', async (t) => {
  const tv = await mountTv(t, {
    portal: { capture: { kind: 'device', path: '/dev/video0' }, output: wled },
    supervisor: {
      preferences: {
        instance: 'http://192.168.1.20:8787',
        receiver: '192.168.1.20:7380',
      },
    },
  });
  assert.match(
    tv.text('lighting-detail'),
    /takes its picture from a video device/,
  );
  await tv.unmount();
  const old = await mountTv(t, {
    supported: false,
    supervisor: { preferences: { instance: 'http://192.168.1.20:8787' } },
  });
  assert.match(old.text('lighting-status'), /too old/);
  assert.deepEqual(old.urls, [], 'no HTTP client is created');
});

test('full: the TV portal is used over loopback and its address is shown', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  assert.deepEqual(tv.urls, ['http://127.0.0.1:8787']);
  assert.match(tv.text('portal-address'), /http:\/\/192\.168\.1\.5:8787\//);
  assert.equal(tv.text('status-title'), 'Lights off');
  assert.equal(tv.el('instance'), null, 'no slim rows');
  assert.equal(tv.el('capture'), null, 'capture is set in the portal only');
  await tv.click('primary');
  assert.deepEqual(
    tv.methods().filter((m) => m !== 'getState'),
    ['setEnabled'],
  );
  assert.equal(tv.text('status-title'), 'Screen sync is on');
  assert.equal(tv.text('primary'), 'Turn lights off');
});

test('full: the lights card says why the lights do not follow the picture', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  await tv.click('mode-processed');
  await act(async () =>
    tv.application.publish({
      session: { state: 'running' },
      input: { fps: 30, receiving: true },
      output_issue: 'other_sender',
    }),
  );
  assert.equal(tv.text('lighting-status'), 'Screen sync is running.');
  assert.match(
    tv.text('lighting-detail'),
    /Another program is sending colours/,
  );
  assert.doesNotMatch(tv.el('lighting-detail').className, /visually-hidden/);
  assert.match(document.querySelector('.card.lighting').className, /attention/);
  await act(async () => tv.application.publish({ output_issue: undefined }));
  assert.match(tv.text('lighting-detail'), /Picture arriving at 30\.0/);
  assert.match(
    tv.el('lighting-detail').className,
    /visually-hidden/,
    'the capture rate above already says it',
  );
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
        ? ['autostart', 'calibrate']
        : ['instance', 'receiver', 'autostart', 'calibrate'],
    );
    assert.doesNotMatch(
      document.body.textContent,
      /fps|× 360|Video only|WLED address|LEDs/,
    );
    assert.match(tv.text('portal-address'), /set up capture and lights/);
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
      'Prysm on this TV',
    );
    assert.equal(tv.text('lighting-title'), 'Your lights');
    assert.equal(
      tv.el('portal-address').querySelector('code').textContent,
      flavor === 'full' ? 'http://192.168.1.5:8787/' : linked.instance,
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
      flavor === 'full' ? 'http://192.168.1.5:8787/' : linked.instance;
    assert.equal(tv.text('status-title'), 'Set up your lights');
    assert.equal(
      tv.text('status-description'),
      `Open ${address} on your phone or computer to set up your lights.`,
    );
    assert.match(
      tv.el('status-title').closest('.overview').className,
      /attention/,
    );
    assert.match(
      document.querySelector('.card.lighting').textContent,
      /Set up your lights in the web portal first/,
    );
    // Once the portal has lights, the usual status returns.
    await act(async () =>
      tv.application.publish({
        settings: { ...tv.application.snapshot().settings, output: wled },
      }),
    );
    await settle();
    assert.notEqual(tv.text('status-title'), 'Set up your lights');
    await tv.unmount();
  }
});

test('the lights setup hint gives way to failures and to a missing receiver', () => {
  const base = {
    ...state(),
    worker: { phase: 'running' },
    power: 'active',
    video: 'available',
    capturing: false,
    failure: null,
  };
  const noLights = { lights: false, address: 'http://192.168.1.20:8787' };
  const draft = completePreferences({});
  const status = (value) => tvStatus(true, value, draft, false, noLights);
  assert.equal(
    status({ ...base, flavor: 'full', enabled: false }).title,
    'Set up your lights',
  );
  assert.equal(
    status({ ...base, flavor: 'full', enabled: false }).action,
    'none',
    'screen sync needs lights',
  );
  const slimOff = {
    ...base,
    flavor: 'slim',
    enabled: false,
    preferences: { receiver: '192.168.1.20:7380' },
  };
  assert.equal(
    tvStatus(
      true,
      slimOff,
      completePreferences(slimOff.preferences),
      false,
      noLights,
    ).action,
    'enable',
    'capture does not',
  );
  assert.equal(
    status({
      ...base,
      flavor: 'full',
      failure: { kind: 'other', message: 'worker exited 1' },
    }).title,
    'Capture needs attention',
  );
  assert.equal(
    status({
      ...base,
      flavor: 'full',
      worker: { phase: 'retrying', seconds: 4 },
      failure: { kind: 'other', message: 'x' },
    }).title,
    'Capture needs attention',
  );
  assert.equal(
    status({ ...base, flavor: 'slim', preferences: {} }).title,
    'Not set up',
  );
  assert.equal(
    status({
      ...base,
      flavor: 'slim',
      preferences: { receiver: '192.168.1.20:7380' },
    }).title,
    'Set up your lights',
  );
  assert.equal(
    tvStatus(true, { ...base, flavor: 'full', enabled: false }, draft, false, {
      lights: true,
      address: null,
    }).title,
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
  instance: 'http://192.168.1.20:8787',
  receiver: '192.168.1.20:7380',
};

test('slim: the status card shows the encoding in use while streaming', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: linked,
      state: { enabled: true, worker: { phase: 'running' } },
    },
  });
  assert.equal(
    tv.el('encoding-in-use'),
    null,
    'nothing is in use before the receiver is connected',
  );
  await act(async () =>
    tv.supervisor.publish({ capturing: true, encoding: 'h264_hardware' }),
  );
  await settle();
  assert.equal(tv.text('status-title'), 'Sending the picture');
  assert.equal(tv.text('encoding-in-use'), 'Encoding in use: Hardware H.264');
  await act(async () => tv.supervisor.publish({ encoding: 'raw' }));
  await settle();
  assert.equal(tv.text('encoding-in-use'), 'Encoding in use: Uncompressed');
  await act(async () =>
    tv.supervisor.publish({ capturing: false, encoding: null }),
  );
  await settle();
  assert.equal(tv.el('encoding-in-use'), null);
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
        failure: { kind: 'encoder', message: failure },
        encoding: 'h264_hardware',
      },
    },
  });
  assert.equal(tv.text('status-title'), 'Capture needs attention');
  assert.equal(
    tv.text('status-description'),
    `${failure}. Choose another stream encoding under Capture in Prysm’s web portal. Open Service options for diagnostics.`,
  );
  assert.equal(tv.text('encoding-in-use'), 'Encoding in use: Hardware H.264');
  assert.equal(
    tv.el('portal-address').querySelector('code').textContent,
    linked.instance,
  );
  assert.equal(tv.el('encoding'), null, 'no encoding setting on the TV');
});

test('slim: a receiver wait or another failure does not mention the encoding', async (t) => {
  const wait = await mountTv(t, {
    supervisor: {
      preferences: linked,
      state: {
        enabled: true,
        worker: { phase: 'running' },
        failure: {
          kind: 'receiver',
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
        worker: { phase: 'retrying', seconds: 4 },
        failure: { kind: 'other', message: 'worker exited 1: capture failed' },
      },
    },
  });
  assert.equal(tv.text('status-title'), 'Capture needs attention');
  assert.equal(
    tv.text('status-description'),
    'worker exited 1: capture failed. Retrying in 4 s.',
  );
  await tv.key(DOWN);
  assert.equal(tv.focus(), 'service-options');
});

test('full: no encoding setting, encoding in use or encoding switch', async (t) => {
  const tv = await mountTv(t, {
    flavor: 'full',
    supervisor: {
      state: {
        worker: { phase: 'retrying', seconds: 4 },
        failure: { kind: 'encoder', message: 'worker exited 1' },
        encoding: 'h264_hardware',
      },
    },
  });
  assert.equal(tv.text('status-title'), 'Capture needs attention');
  assert.equal(tv.el('encoding'), null);
  assert.equal(tv.el('encoding-in-use'), null);
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
    instance: 'http://192.168.1.20:8787',
    receiver: '192.168.1.20:7380',
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
    assert.equal(tv.text('calibrate-value'), 'Calibration needs a newer TV');
    await tv.unmount();
  }
  const old = await mountTv(t, {
    flavor: 'full',
    patterns: fakePlayer(),
    supported: false,
  });
  assert.equal(old.text('calibrate-value'), 'Calibration needs a newer TV');
});

test('calibration is disabled with a reason while the lights are not driven from this TV', async (t) => {
  const cases = [
    [{}, 'Add the Prysm address first'],
    [
      { supervisor: { preferences: slimReady.preferences } },
      'Start capture first',
    ],
    [{ supervisor: slimReady, block: true }, 'Needs a connection to Prysm'],
    [
      {
        supervisor: slimReady,
        portal: {
          capture: { kind: 'device', path: '/dev/video0' },
          output: wled,
        },
      },
      'Prysm is not using this TV’s picture',
    ],
    [
      { flavor: 'full', portal: { capture: portals.full.capture } },
      'Set up capture and lights first',
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
    ['target:processed'],
  );
  assert.equal(tv.el('app'), null, 'the main screen is closed');
  assert.equal(
    tv.text('calibration-address'),
    'http://192.168.1.5:8787/#/calibrate',
  );
  assert.equal(tv.text('calibration-hint'), 'Scan with your phone to start.');
  assert(tv.el('calibration-qr'));
  // The gray pattern already plays under the waiting screen.
  assert.equal(player.plays, 1);
  assert.equal(player.video, tv.el('pattern-video'));
  assert.equal(player.pattern(), 'gray-192');

  await phone(tv, { pattern: 'red', uncorrected: false });
  assert.equal(visibleText(), '', 'nothing but the video');
  assert.equal(document.querySelector('svg'), null);
  assert(tv.el('pattern-video'));
  assert.equal(player.pattern(), 'red');
  await phone(tv, { pattern: 'blue-pair', uncorrected: true });
  assert.equal(player.pattern(), 'blue-pair');
  assert.equal(player.plays, 1, 'one stream for every pattern');
  assert.equal(player.stops, 0);

  // The phone ends calibration: the TV waits for it again.
  await phone(tv, null);
  assert.equal(
    tv.text('calibration-address'),
    'http://192.168.1.5:8787/#/calibrate',
  );
  assert.equal(player.pattern(), 'gray-192');
  assert.equal(player.plays, 1);

  // Back leaves: the pattern ends, lighting stays on.
  const before = tv.application.calls.length;
  await tv.key(BACK);
  assert.deepEqual(tv.application.calls.slice(before), ['calibration:end']);
  assert.equal(player.stops, 1);
  assert(tv.el('app'));
  assert.equal(tv.focus(), 'calibrate');
  assert.equal(tv.el('modal'), null, 'Back does not ask to close the app');
  assert(!tv.application.calls.includes('target:off'));
  assert.equal(tv.application.snapshot().target, 'processed');
});

test('slim: calibration uses the computer portal, already on, and ends when the app is hidden', async (t) => {
  const player = fakePlayer();
  const tv = await mountTv(t, { patterns: player, supervisor: slimReady });
  await act(async () => tv.application.client.setTarget('processed'));
  await settle();
  const before = tv.application.calls.length;
  await tv.click('calibrate');
  assert(
    !tv.application.calls
      .slice(before)
      .some((call) => call.startsWith('target:')),
    'already on',
  );
  assert.equal(
    tv.text('calibration-address'),
    'http://192.168.1.20:8787/#/calibrate',
  );
  await phone(tv, { pattern: 'coast', uncorrected: false });
  assert.equal(player.pattern(), 'coast');
  Object.defineProperty(document, 'hidden', {
    value: true,
    configurable: true,
  });
  await act(async () =>
    document.dispatchEvent(new tv.dom.window.Event('visibilitychange')),
  );
  await settle();
  assert.equal(tv.application.calls.at(-1), 'calibration:end');
  assert.equal(player.stops, 1);
  assert(tv.el('app'));
  assert.equal(tv.application.snapshot().calibration, undefined);
  assert.equal(tv.application.snapshot().target, 'processed');
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
  assert.equal(tv.text('notice'), 'Capture could not open');
  assert.equal(player.plays, 0);

  await tv.click('calibrate');
  await phone(tv, { pattern: 'green', uncorrected: false });
  await act(async () => player.failed());
  await settle();
  assert.equal(
    tv.text('calibration-error'),
    'The pattern video stopped. Press Back and try again.',
  );
  assert.equal(tv.el('pattern-video'), null);
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

test('the pattern fragments are the shipped clips, one per pattern of the contract', () => {
  const fragments = JSON.parse(
    readFileSync(
      resolve(process.cwd(), 'packages/tv-ui/pattern-fragments.json'),
      'utf8',
    ),
  );
  const root = JSON.parse(
    readFileSync(
      resolve(
        process.cwd(),
        '../../crates/web/prysm-webos/src/pattern-fragments.json',
      ),
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
    assert.equal(fragment, root[name], name);
});

test('the pattern stream appends the requested pattern in sequence mode and stops on a stall', (t) => {
  t.mock.timers.enable({ apis: ['setInterval', 'Date'] });
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
  let pattern = 'gray-192';
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
    assert.deepEqual(appended, bytes('gray-192'));
  // Playing: a new pattern is appended when less than half a second is left.
  pattern = 'red';
  video.currentTime = 1.5;
  buffer.dispatchEvent(new Event('updateend'));
  assert.equal(buffer.appended.length, 5);
  video.currentTime = 2.2;
  buffer.dispatchEvent(new Event('updateend'));
  assert.equal(buffer.appended.length, 6);
  assert.deepEqual(buffer.appended.at(-1), bytes('red'));
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
