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
  derivedReceiver,
  instanceAddress,
  listenPort,
  localPortal,
  receiverAddress,
  tvStatus,
  completePreferences,
} from '../packages/tv-ui/model';
import { fakeApplicationPort } from '../testkit/application';
import { fakeTvSupervisor } from '../testkit/luna';

const fixtures = JSON.parse(
  readFileSync(resolve(process.cwd(), 'packages/contracts/generated/fixtures.json'), 'utf8'),
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
  full: { capture: { kind: 'webos', width: 640, height: 360, fps: 30 }, output: wled },
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
              Promise.reject(new ApplicationError({ kind: 'unavailable', message: 'Cannot reach the portal.' })),
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
          new dom.window.KeyboardEvent('keydown', { keyCode: code, bubbles: true, cancelable: true, ...extra }),
        ),
      );
      await settle();
    },
    choose: async (label) => {
      const choice = [...document.querySelectorAll('#dialog-body button')].find((item) =>
        item.textContent.startsWith(label),
      );
      assert(choice, `no choice ${label}`);
      await act(async () => choice.click());
      await settle();
    },
    type: async (id, value) => {
      const input = el(id);
      await act(async () => {
        Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, 'value').set.call(input, value);
        input.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
      });
    },
    submit: async (id) => {
      await act(async () =>
        el(id).closest('form').dispatchEvent(new dom.window.Event('submit', { bubbles: true, cancelable: true })),
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
      reply: (value) => channel.onservicecallback(typeof value === 'string' ? value : JSON.stringify(value)),
    };
    channels.push(channel);
    return channel;
  };
  return { factory, channels };
}
const state = (change = {}) => ({ ...fixtures['luna/State'][0], ...change });

test('generated Rust states are accepted; contract violations are not', () => {
  for (const value of fixtures['luna/State']) assert(isState(value), JSON.stringify(value));
  assert(!isState({ ...state(), flavor: 'lite' }));
  assert(!isState({ ...state(), worker: 'idle' }));
  assert(!isState({ ...state(), log: [1] }));
  assert(!isState(null));
});

test('the state subscription retries after unsubscribed, malformed and refused replies', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const bridge = manualBridge();
  const events = [];
  const stop = lunaTvService(bridge.factory).watch((event) => events.push(event));
  assert.equal(bridge.channels[0].uri, 'luna://io.prysm.tv.service/getState');
  assert.deepEqual(bridge.channels[0].payload, { subscribe: true });
  bridge.channels[0].reply({ returnValue: true, subscribed: false, ...state() });
  assert.equal(events.at(-1).online, false);
  assert.equal(bridge.channels[0].cancelled, 1);
  assert.equal(events.length, 1, 'the callback during cancel is ignored');
  t.mock.timers.tick(RESUBSCRIBE_MS);
  bridge.channels[1].reply('not json');
  assert.equal(events.at(-1).error.kind, 'invalid_reply');
  t.mock.timers.tick(RESUBSCRIBE_MS);
  bridge.channels[2].reply({ returnValue: true, subscribed: true, ...state({ worker: 'idle' }) });
  assert.equal(events.at(-1).error.kind, 'invalid_reply');
  t.mock.timers.tick(RESUBSCRIBE_MS);
  bridge.channels[3].reply({ returnValue: true, subscribed: true, ...state({ enabled: false }) });
  bridge.channels[3].reply({ returnValue: true, ...state({ enabled: true }) });
  assert.deepEqual(events.slice(-2).map((event) => event.online && event.state.enabled), [false, true]);
  assert.equal('returnValue' in events.at(-1).state, false);
  bridge.channels[3].reply({ returnValue: false, errorText: 'Service stopped' });
  assert.equal(events.at(-1).error.kind, 'refused');
  stop();
  t.mock.timers.tick(RESUBSCRIBE_MS);
  assert.equal(bridge.channels.length, 4, 'a stopped watch does not resubscribe');
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
  bridge.channels[0].onservicecallback?.(JSON.stringify({ returnValue: true, subscribed: true, ...state() }));
  assert.equal(events.length, 1);
  stop();

  const enabling = service.setEnabled(true);
  t.mock.timers.tick(CALL_TIMEOUT_MS);
  await assert.rejects(enabling, { kind: 'timeout' });
  const channel = bridge.channels.at(-1);
  assert.equal(channel.cancelled, 1);
  // A retry works after the timeout.
  const again = service.setEnabled(true);
  bridge.channels.at(-1).reply({ returnValue: true, ...state({ enabled: true }) });
  assert.equal((await again).enabled, true);
});

test('commands carry the contract payloads and fail with typed errors', async () => {
  const bridge = manualBridge();
  const service = lunaTvService(bridge.factory);
  const saving = service.setPreferences({ autostart: true });
  assert.equal(bridge.channels[0].uri, 'luna://io.prysm.tv.service/setPreferences');
  assert.deepEqual(bridge.channels[0].payload, { autostart: true });
  bridge.channels[0].reply({ returnValue: false, errorText: 'only io.prysm.tv may change Prysm' });
  await assert.rejects(saving, { kind: 'refused', message: 'only io.prysm.tv may change Prysm' });
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
  await assert.rejects(thrown.setEnabled(true), { kind: 'unavailable', message: 'Bridge call failed' });
  await assert.rejects(lunaTvService(null).setEnabled(true), { kind: 'unavailable' });
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
  assert.equal(SETUP_SCRIPT, '/media/developer/apps/usr/palm/services/io.prysm.tv.service/setup.sh');
  bridge.channels[0].reply({ returnValue: true, stdoutString: '' });
  await ok;
  const failing = service.setup();
  bridge.channels[1].reply({ returnValue: false, stderrString: 'setup: cannot write manifests' });
  await assert.rejects(failing, { kind: 'refused', message: 'setup: cannot write manifests' });
  const missing = service.setup();
  bridge.channels[2].reply({ returnValue: false, errorText: 'Unknown service name: org.webosbrew.hbchannel.service' });
  await assert.rejects(missing, { kind: 'unavailable', message: /Homebrew Channel/ });
});

/* ---- Rules ---- */

test('addresses: instance origins, receivers and the derived receiver port', () => {
  assert.deepEqual(instanceAddress('192.168.1.20'), { value: 'http://192.168.1.20:8787' });
  assert.deepEqual(instanceAddress(' prysm.local/ '), { value: 'http://prysm.local:8787' });
  assert.deepEqual(instanceAddress('192.168.1.20:9000'), { value: 'http://192.168.1.20:9000' });
  assert.deepEqual(instanceAddress('HTTPS://prysm.home'), { value: 'https://prysm.home' });
  assert.deepEqual(instanceAddress('[fd00::2]'), { value: 'http://[fd00::2]:8787' });
  assert.deepEqual(instanceAddress(''), { value: null });
  assert('error' in instanceAddress('http://prysm.local/portal'));
  assert('error' in instanceAddress('ftp://prysm'));
  assert.deepEqual(receiverAddress('192.168.1.20:7380'), { value: '192.168.1.20:7380' });
  assert('error' in receiverAddress('prysm.local:7380'), 'receivers are socket addresses');
  assert('error' in receiverAddress('192.168.1.20'));
  assert.equal(listenPort({ kind: 'remote', listen: '0.0.0.0:7380' }), 7380);
  assert.equal(listenPort({ kind: 'device', path: '/dev/video0' }), null);
  assert.equal(derivedReceiver('http://192.168.1.20:8787', 7380), '192.168.1.20:7380');
  assert.equal(derivedReceiver('http://prysm.local:8787', 7380), null);
  assert.equal(localPortal('http://192.168.1.5:8787/'), 'http://127.0.0.1:8787');
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
    worker: 'running',
    power: 'active',
    video: 'available',
    capturing: false,
    failure: null,
    ...change,
  });
  const draft = (value = {}) => completePreferences({ receiver: '192.168.1.20:7380', ...value });
  const title = (online, value, d = draft(), dirty = false) => tvStatus(online, value, d, dirty).title;
  assert.equal(title(null, null), 'Connecting…');
  assert.equal(tvStatus(false, null, draft(), false).label, 'Set up service');
  assert.equal(title(true, slim({ capturing: true })), 'Sending the picture');
  assert(tvStatus(true, slim({ capturing: true }), draft(), false).ready);
  assert.equal(title(true, slim({ power: 'asleep' })), 'Waiting for TV');
  assert.equal(title(true, slim({ video: 'unavailable' })), 'Waiting for video');
  const waiting = tvStatus(true, slim({ failure: 'waiting for the receiver: cannot connect to receiver: refused' }), draft(), false);
  assert.equal(waiting.title, 'Waiting for Prysm');
  assert.equal(waiting.attention, false, 'waiting is not a failure');
  const retrying = tvStatus(true, slim({ worker: 'retrying', failure: 'worker exited 1' }), draft(), false);
  assert.equal(retrying.title, 'Capture needs attention');
  assert.match(retrying.description, /worker exited 1\. Prysm tries again/);
  assert.equal(title(true, slim({ failure: 'capture failed' })), 'Capture needs attention');
  assert.equal(title(true, slim({ worker: 'stopping' })), 'Stopping capture…');
  const notSet = tvStatus(true, slim({ enabled: false }, {}), completePreferences({}), false);
  assert.deepEqual([notSet.title, notSet.label, notSet.action], ['Not set up', 'Set Prysm address', 'address']);
  assert.equal(tvStatus(true, slim({ enabled: false }), draft({ autostart: true }), true).label, 'Save and enable');
  assert.equal(tvStatus(true, slim({ enabled: false }), draft(), false).label, 'Enable');
  assert.equal(tvStatus(true, slim(), draft(), false).label, 'Disable');
  const full = (change) => ({ ...state(), flavor: 'full', worker: 'running', power: 'active', video: 'available', failure: null, ...change });
  assert.deepEqual(
    [tvStatus(true, full({ enabled: false, lighting: 'off' }), draft(), false)].map((s) => [s.title, s.label]),
    [['Lights off', 'Start screen sync']],
  );
  assert.equal(title(true, full({ enabled: true, lighting: 'processed', capturing: true })), 'Screen sync is on');
  assert.equal(title(true, full({ enabled: true, lighting: 'device_native' })), 'Light effects on');
  assert.equal(tvStatus(true, full({ enabled: true }), draft(), false).label, 'Turn lights off');
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
  const exec = tv.supervisor.calls.find((call) => call.uri.startsWith('luna://org.webosbrew'));
  assert.deepEqual(exec.payload, { command: SETUP_SCRIPT });
  assert.equal(tv.text('status-title'), 'Not set up');
});

test('remote arrows move focus through rows and columns without changing settings', async (t) => {
  const tv = await mountTv(t, { supervisor: { preferences: { instance: 'http://192.168.1.20:8787', receiver: '192.168.1.20:7380' } } });
  assert.equal(tv.focus(), 'primary');
  await tv.key(DOWN);
  assert.equal(tv.focus(), 'service-options');
  await tv.key(DOWN);
  assert.equal(tv.focus(), 'mode-off', 'the lighting row is one stop, entered at its chosen mode');
  await tv.key(LEFT);
  assert.equal(tv.focus(), 'mode-device_native');
  await tv.key(RIGHT);
  await tv.key(RIGHT);
  assert.equal(tv.focus(), 'instance', 'the row edge continues to the settings column');
  await tv.key(DOWN);
  await tv.key(DOWN);
  assert.equal(tv.focus(), 'size');
  await tv.key(LEFT);
  assert.equal(tv.focus(), 'mode-off', 'left returns to the control last used there');
  await tv.key(RIGHT);
  assert.equal(tv.focus(), 'size');
  await tv.key(UP);
  await tv.key(UP);
  await tv.key(UP);
  assert.equal(tv.focus(), 'instance');
  assert(!tv.methods().includes('setPreferences'));
  assert.equal(tv.text('saved-state'), 'Saved');
});

test('OK opens choices, arrows only focus, Back cancels, and Save alone persists', async (t) => {
  const tv = await mountTv(t, { supervisor: { preferences: { instance: 'http://192.168.1.20:8787', receiver: '192.168.1.20:7380' } } });
  tv.el('rate').focus();
  await tv.click('rate');
  assert.equal(tv.text('dialog-title'), 'Frame rate');
  assert.equal(document.activeElement.getAttribute('aria-pressed'), 'true', 'the current choice is focused');
  await tv.key(DOWN);
  await tv.key(BACK);
  assert.equal(tv.el('modal'), null);
  assert.equal(tv.focus(), 'rate', 'focus returns to the row');
  assert.equal(tv.text('rate-value'), '30 fps');
  await tv.click('rate');
  await tv.choose('50 fps');
  assert.equal(tv.text('rate-value'), '50 fps');
  assert.equal(tv.text('saved-state'), 'Unsaved changes');
  assert(!tv.methods().includes('setPreferences'), 'choosing does not save');
  await tv.click('save');
  const saved = tv.supervisor.calls.find((call) => call.method === 'setPreferences').payload;
  assert.equal(saved.capture.fps, 50);
  assert.equal(saved.instance, 'http://192.168.1.20:8787');
  assert.equal(tv.text('saved-state'), 'Saved');
  assert.equal(tv.supervisor.state().preferences.capture.fps, 50);
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

test('the receiver is derived from the remote capture port, then Save and enable sends it first', async (t) => {
  const tv = await mountTv(t);
  await tv.click('primary');
  await tv.type('instance-input', '192.168.1.20');
  await tv.submit('instance-input');
  await settle();
  assert.deepEqual(tv.urls, ['http://192.168.1.20:8787']);
  assert.equal(tv.text('receiver-value'), '192.168.1.20:7380');
  assert.match(tv.text('notice'), /Frames will go to 192\.168\.1\.20:7380/);
  assert.equal(tv.text('primary'), 'Save and enable');
  await tv.click('primary');
  await settle();
  assert.deepEqual(tv.methods().filter((m) => m !== 'getState'), ['setPreferences', 'setEnabled']);
  assert.deepEqual(
    [tv.supervisor.state().preferences.instance, tv.supervisor.state().preferences.receiver],
    ['http://192.168.1.20:8787', '192.168.1.20:7380'],
  );
  assert.equal(tv.text('primary'), 'Disable');
});

test('a failed save keeps the draft and never enables', async (t) => {
  const tv = await mountTv(t, { supervisor: { preferences: { instance: 'http://192.168.1.20:8787', receiver: '192.168.1.20:7380' } } });
  tv.supervisor.failNext('setPreferences', 'receiver: needs a host and a fixed port');
  await tv.click('autostart');
  await tv.choose('On');
  await tv.click('primary');
  assert.equal(tv.text('notice'), 'receiver: needs a host and a fixed port');
  assert(!tv.methods().includes('setEnabled'));
  assert.equal(tv.text('autostart-value'), 'On');
  assert.equal(tv.text('saved-state'), 'Unsaved changes');
});

test('status updates keep unsaved choices and focus; a change made elsewhere replaces the edit', async (t) => {
  const tv = await mountTv(t, { supervisor: { preferences: { instance: 'http://192.168.1.20:8787', receiver: '192.168.1.20:7380' } } });
  await tv.click('size');
  await tv.choose('640 × 360');
  tv.el('rate').focus();
  await act(async () => tv.supervisor.publish({ capturing: true, enabled: true, worker: 'running' }));
  await settle();
  assert.equal(tv.text('size-value'), '640 × 360');
  assert.equal(tv.focus(), 'rate');
  const prefs = tv.supervisor.state().preferences;
  await act(async () => tv.supervisor.publish({ preferences: { ...prefs, capture: { ...prefs.capture, width: 480, height: 270 } } }));
  await settle();
  assert.equal(tv.text('size-value'), '480 × 270');
  assert.match(tv.text('notice'), /changed elsewhere/);
  assert.equal(tv.text('saved-state'), 'Saved');
});

test('removing the setup is tucked away, needs confirmation, and Back returns one level', async (t) => {
  const tv = await mountTv(t, { supervisor: { preferences: { receiver: '192.168.1.20:7380' } } });
  tv.el('service-options').focus();
  await tv.click('service-options');
  await tv.click('cleanup');
  assert.equal(tv.text('dialog-title'), 'Remove service setup?');
  await tv.key(BACK);
  assert.equal(tv.text('dialog-title'), 'Service options');
  await tv.click('diagnostics');
  assert.match(tv.text('diagnostics-text'), /Frames go to: 192\.168\.1\.20:7380/);
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
  assert(!tv.methods().includes('setEnabled'), 'closing leaves capture as it is');
});

test('hiding the page releases the subscriptions; showing it subscribes again', async (t) => {
  const tv = await mountTv(t, { supervisor: { preferences: { instance: 'http://192.168.1.20:8787', receiver: '192.168.1.20:7380' } } });
  assert.equal(tv.supervisor.subscribers(), 1);
  assert(tv.application.calls.includes('observe'));
  Object.defineProperty(document, 'hidden', { value: true, configurable: true });
  await act(async () => document.dispatchEvent(new tv.dom.window.Event('visibilitychange')));
  await settle();
  assert.equal(tv.supervisor.subscribers(), 0);
  assert(tv.application.calls.includes('unsubscribe'));
  Object.defineProperty(document, 'hidden', { value: false, configurable: true });
  await act(async () => document.dispatchEvent(new tv.dom.window.Event('visibilitychange')));
  await settle();
  assert.equal(tv.supervisor.subscribers(), 1);
  assert.equal(tv.application.calls.filter((call) => call === 'observe').length, 2);
});

test('a lost subscription recovers after 3 s without losing the draft', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const tv = await mountTv(t, { supervisor: { preferences: { instance: 'http://192.168.1.20:8787', receiver: '192.168.1.20:7380' } } });
  await tv.click('source');
  await tv.choose('Video only');
  assert.equal(tv.text('source-value'), 'Video only');
  await act(async () => tv.supervisor.drop('Service stopped'));
  await settle();
  assert.equal(tv.text('status-title'), 'TV service unavailable');
  assert.equal(tv.text('notice'), 'Service stopped');
  assert.equal(tv.text('source-value'), 'Video only');
  await act(async () => t.mock.timers.tick(RESUBSCRIBE_MS));
  await settle();
  assert.equal(tv.text('status-title'), 'Disabled');
  assert.equal(tv.text('notice'), 'Service connected.');
  assert.equal(tv.text('source-value'), 'Video only');
  assert.equal(tv.text('saved-state'), 'Unsaved changes');
});

test('a waiting slim worker is described as waiting for Prysm, not as a failure', async (t) => {
  const tv = await mountTv(t, {
    supervisor: {
      preferences: { instance: 'http://192.168.1.20:8787', receiver: '192.168.1.20:7380' },
      state: { enabled: true, worker: 'running', failure: 'waiting for the receiver: cannot connect to receiver: refused' },
    },
  });
  assert.equal(tv.text('status-title'), 'Waiting for Prysm');
  assert.match(tv.text('status-description'), /192\.168\.1\.20:7380/);
  assert.equal(tv.text('primary'), 'Disable');
});

test('slim lighting buttons drive the remote Prysm; leaving never turns it off', async (t) => {
  const tv = await mountTv(t, { supervisor: { preferences: { instance: 'http://192.168.1.20:8787', receiver: '192.168.1.20:7380' } } });
  assert.equal(tv.text('lighting-status'), 'Lights are off.');
  await tv.click('mode-processed');
  assert.equal(tv.application.snapshot().target, 'processed');
  assert.equal(tv.text('lighting-status'), 'Screen sync is on.');
  assert.equal(tv.el('mode-processed').getAttribute('aria-pressed'), 'true');
  assert.equal(tv.text('mode-device_native'), 'WLED effects');
  tv.application.failNext({ kind: 'adapter', message: 'WLED cannot be reached' });
  await tv.click('mode-device_native');
  assert.match(tv.text('lighting-title') + document.querySelector('.lighting').textContent, /WLED cannot be reached/);
  await tv.unmount();
  assert(tv.application.calls.includes('unsubscribe'));
  assert.equal(tv.application.snapshot().target, 'processed');
  assert.equal(tv.supervisor.subscribers(), 0);
});

test('a Prysm that refuses the TV origin says how to allow it; an unreachable one says so', async (t) => {
  const refused = await mountTv(t, {
    block: true,
    probe: () => Promise.resolve(true),
    supervisor: { preferences: { instance: 'http://192.168.1.20:8787', receiver: '192.168.1.20:7380' } },
  });
  await settle();
  assert.match(refused.text('lighting-status'), /does not allow control from the TV/);
  assert.match(refused.text('lighting-detail'), /--allow-origin '\*'/);
  assert(refused.el('mode-processed').closest('[hidden]'), 'no modes without a state');
  await refused.unmount();
  const unreachable = await mountTv(t, {
    block: true,
    probe: () => Promise.resolve(false),
    supervisor: { preferences: { instance: 'http://192.168.1.20:8787', receiver: '192.168.1.20:7380' } },
  });
  await settle();
  assert.match(unreachable.text('lighting-status'), /Cannot reach Prysm at http:\/\/192\.168\.1\.20:8787/);
});

test('a remote Prysm without Remote capture is explained; an old browser gets the portal address', async (t) => {
  const tv = await mountTv(t, {
    portal: { capture: { kind: 'device', path: '/dev/video0' }, output: wled },
    supervisor: { preferences: { instance: 'http://192.168.1.20:8787', receiver: '192.168.1.20:7380' } },
  });
  assert.match(tv.text('lighting-detail'), /takes its picture from a video device/);
  await tv.unmount();
  const old = await mountTv(t, {
    supported: false,
    supervisor: { preferences: { instance: 'http://192.168.1.20:8787' } },
  });
  assert.match(old.text('lighting-status'), /too old/);
  assert.deepEqual(old.urls, [], 'no HTTP client is created');
});

test('calibration is shown as not available yet', async (t) => {
  const tv = await mountTv(t);
  assert(tv.el('calibrate').disabled);
  assert.match(tv.text('calibrate'), /Not available/);
});

test('full: the TV portal is used over loopback and its address is shown', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  assert.deepEqual(tv.urls, ['http://127.0.0.1:8787']);
  assert.match(tv.text('portal-address'), /http:\/\/192\.168\.1\.5:8787\//);
  assert.equal(tv.text('status-title'), 'Lights off');
  assert.equal(tv.el('instance'), null, 'no slim rows');
  assert.equal(tv.text('lights-value'), '192.168.1.50 · 300 LEDs');
  assert.equal(tv.text('capture-value'), '640 × 360, 30 fps, with menus');
  await tv.click('primary');
  assert.deepEqual(tv.methods().filter((m) => m !== 'getState'), ['setEnabled']);
  assert.equal(tv.text('status-title'), 'Screen sync is on');
  assert.equal(tv.text('primary'), 'Turn lights off');
});

test('full: lights are saved explicitly, keep unknown fields and reload when changed elsewhere', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  await tv.click('lights');
  assert.equal(tv.focus(), 'wled-address');
  await tv.key(DOWN);
  assert.equal(tv.focus(), 'wled-top');
  await tv.type('wled-top', '100');
  await tv.type('wled-address', ' http://192.168.1.60/ ');
  // Someone saves the output in the portal meanwhile.
  await act(async () =>
    tv.application.publish({
      revision: 7,
      settings: { ...tv.application.snapshot().settings, output: { ...wled, address: '192.168.1.70' } },
    }),
  );
  await settle();
  assert.equal(tv.el('wled-address').value, ' http://192.168.1.60/ ', 'the open dialog keeps its draft');
  await tv.submit('wled-address');
  assert.match(tv.text('wled-error'), /changed elsewhere/);
  assert.equal(tv.el('wled-address').value, '192.168.1.70', 'the dialog reloaded');
  assert(!tv.application.calls.includes('replaceSettings'));
  await tv.type('wled-top', '100');
  await tv.submit('wled-top');
  assert.equal(tv.el('modal'), null);
  assert.equal(tv.text('notice'), 'Lights saved.');
  const output = tv.application.snapshot().settings.output;
  assert.deepEqual(output, { ...wled, address: '192.168.1.70', leds: { ...wled.leds, top: 100 } });
  assert.equal(tv.text('lights-value'), '192.168.1.70 · 304 LEDs');
});

test('full: invalid lights are explained before and after sending', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  await tv.click('lights');
  for (const edge of ['top', 'right', 'bottom', 'left']) await tv.type(`wled-${edge}`, '0');
  await tv.submit('wled-top');
  assert.match(tv.text('wled-error'), /At least one LED/);
  assert.equal(tv.focus(), 'wled-top');
  await tv.type('wled-top', '10');
  tv.application.failNext({ kind: 'invalid', field: 'output.address', message: 'a unicast device address is required' });
  await tv.submit('wled-top');
  assert.equal(tv.text('wled-error'), 'a unicast device address is required');
  assert.equal(tv.focus(), 'wled-address');
  assert.equal(tv.el('wled-address').getAttribute('aria-invalid'), 'true');
});

test('full: capture size, rate and menus are chosen with the remote and saved together', async (t) => {
  const tv = await mountTv(t, { flavor: 'full' });
  await tv.click('capture');
  assert.equal(document.activeElement.textContent, '640 × 360');
  await tv.key(RIGHT);
  assert.equal(document.activeElement.textContent, '960 × 540');
  await act(async () => document.activeElement.click());
  await tv.key(DOWN);
  assert.equal(document.activeElement.textContent, '30 fps', 'down enters the next row at its choice');
  await tv.key(LEFT);
  await act(async () => document.activeElement.click());
  await tv.key(DOWN);
  await tv.key(RIGHT);
  await act(async () => document.activeElement.click());
  await tv.key(DOWN);
  assert.equal(tv.focus(), 'capture-save');
  await tv.click('capture-save');
  assert.deepEqual(tv.application.snapshot().settings.capture, {
    kind: 'webos',
    width: 960,
    height: 540,
    fps: 25,
    include_ui: false,
  });
  assert.equal(tv.text('capture-value'), '960 × 540, 25 fps');
});

test('TV bundles are classic deferred scripts with relative URLs and no demo code', () => {
  for (const host of ['webos', 'webos-slim']) {
    const dir = resolve(process.cwd(), 'dist', host, 'tv-ui');
    const html = readFileSync(resolve(dir, 'index.html'), 'utf8');
    assert.match(html, /<script defer src="\.\/assets\/[^"]+\.js"><\/script>/);
    assert.match(html, /<link rel="stylesheet" href="\.\/assets\/[^"]+\.css">/);
    assert.doesNotMatch(html, /type="module"|<style|style=/);
    const assets = readdirSync(resolve(dir, 'assets'));
    assert.deepEqual(assets.map((name) => name.split('.').pop()).sort(), ['css', 'js']);
    const script = readFileSync(resolve(dir, 'assets', assets.find((name) => name.endsWith('.js'))), 'utf8');
    assert.doesNotMatch(script, /fakeTvSupervisor|demo:|in-memory/);
    assert.doesNotMatch(script, /\?\.|\?\?|async function/, 'Chromium 53 syntax');
  }
});
