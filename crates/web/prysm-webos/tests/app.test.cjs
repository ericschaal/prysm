'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const { createRequire } = require('node:module');
const path = require('node:path');
const { JSDOM } = require('jsdom');
const ts = require('typescript');
const devService = ts.transpileModule(
  fs.readFileSync(path.join(__dirname, '../src/dev-service.ts'), 'utf8'),
  {
    compilerOptions: {
      target: ts.ScriptTarget.ES2015,
      module: ts.ModuleKind.CommonJS,
    },
  },
).outputText;
const service = ts.transpileModule(
  fs.readFileSync(path.join(__dirname, '../src/service.ts'), 'utf8'),
  {
    compilerOptions: {
      target: ts.ScriptTarget.ES2015,
      module: ts.ModuleKind.CommonJS,
    },
  },
).outputText;
const embeddedBuild = process.env.VITE_EMBEDDED_PRYSM === '1';
const dist = path.resolve(
  __dirname,
  '..',
  process.env.PRYSM_WEBOS_TEST_DIST || 'dist',
);
const html = fs.readFileSync(path.join(dist, 'index.html'), 'utf8');
const script = fs.readFileSync(
  path.join(dist, html.match(/<script[^>]+src="([^"]+)"/)[1]),
  'utf8',
);

async function fixture(t, options = {}) {
  const dom = new JSDOM(html, {
    runScripts: 'outside-only',
    url: 'file:///media/developer/apps/usr/palm/applications/io.prysm.capture/index.html',
  });
  t.after(() => dom.window.close());
  const w = dom.window;
  const tasks = [];
  w.setImmediate = (callback) => {
    tasks.push(callback);
  };
  async function settle(action = () => {}) {
    action();
    await new Promise(setImmediate);
    while (tasks.length) {
      tasks.shift()();
      await new Promise(setImmediate);
    }
  }
  let now = 0;
  let timerId = 0;
  const timers = new Map();
  w.setTimeout = (callback, delay) => {
    const id = ++timerId;
    timers.set(id, { at: now + delay, callback });
    return id;
  };
  w.clearTimeout = (id) => timers.delete(id);
  function advance(milliseconds) {
    const end = now + milliseconds;
    while (true) {
      const next = [...timers.entries()]
        .filter(([, timer]) => timer.at <= end)
        .sort((a, b) => a[1].at - b[1].at)[0];
      if (!next) break;
      timers.delete(next[0]);
      now = next[1].at;
      next[1].callback();
    }
    now = end;
  }
  let subscription;
  const calls = [];
  const pending = [];
  let value = {
    returnValue: true,
    subscribed: false,
    subscribers: 0,
    pid: 42,
    revision: 1,
    videoAvailable: null,
    receiverBusy: false,
    capturing: false,
    enabled: false,
    power: 'Active',
    senderRunning: false,
    receiverConnected: false,
    stopping: false,
    error: null,
    lastLog: null,
    capabilities: {
      hardwareH264: options.hardwareH264 === true,
      ...options.capabilities,
    },
    config: {
      instanceUrl: options.instanceUrl ?? 'http://192.168.18.18:8787',
      source: 'both',
      compression: 'lz4',
      autostart: false,
      metrics: false,
      ...options.config,
    },
  };
  w.PalmServiceBridge = function () {
    this.cancel = () => {
      this.cancelled = true;
      if (options.cancelReentry)
        this.onservicecallback('{"returnValue":false}');
    };
    this.call = (uri, data) => {
      const payload = JSON.parse(data);
      const method = uri.split('/').pop();
      calls.push({ method, payload });
      if (options.throwMethod === method) throw new Error('Bridge call failed');
      if (options.deferMethods?.includes(method)) {
        pending.push(this);
        return;
      }
      if (method === 'status') {
        assert.equal(payload.subscribe, true, 'status must use a subscription');
        subscription = this;
        if (options.silentStatus) return;
      }
      if (method === 'status' && options.offline) {
        this.onservicecallback(
          JSON.stringify({
            returnValue: false,
            errorText: 'Service unavailable',
          }),
        );
        return;
      }
      if (method === 'setConfig') value.config = payload;
      if (method === 'start') {
        value.enabled = true;
        value.senderRunning = true;
      }
      if (method === 'stop' || method === 'cleanup') {
        value.enabled = false;
        value.senderRunning = false;
      }
      if (method === 'cleanup') value.config.autostart = false;
      this.onservicecallback(
        JSON.stringify({ ...value, subscribed: method === 'status' }),
      );
    };
  };
  if (options.devService) {
    delete w.PalmServiceBridge;
    w.eval('var exports = {};\n' + devService);
    w.exports.installDevService();
  }
  w.eval(script);
  await settle();
  return {
    settle,
    w,
    calls,
    options,
    pending,
    subscription: () => subscription,
    advance: (milliseconds) => settle(() => advance(milliseconds)),
    input: (id, value) =>
      settle(() => {
        const input = w.document.getElementById(id);
        Object.getOwnPropertyDescriptor(
          w.HTMLInputElement.prototype,
          'value',
        ).set.call(input, value);
        input.dispatchEvent(new w.Event('input', { bubbles: true }));
      }),
    click: (id) => settle(() => w.document.getElementById(id).click()),
    event: (name) => settle(() => w.dispatchEvent(new w.Event(name))),
    reply: (bridge, text) => settle(() => bridge.onservicecallback(text)),
    notify: () =>
      settle(() =>
        subscription.onservicecallback(
          JSON.stringify({ ...value, subscribed: true }),
        ),
      ),
    get value() {
      return value;
    },
    el: (id) => w.document.getElementById(id),
    focus: () => w.document.activeElement.id,
    key: (code) =>
      settle(() =>
        w.document.activeElement.dispatchEvent(
          new w.KeyboardEvent('keydown', {
            keyCode: code,
            bubbles: true,
            cancelable: true,
          }),
        ),
      ),
    mutations: () => calls.filter((call) => call.method !== 'status'),
  };
}

test('remote arrows move through setting rows without changing any setting', async (t) => {
  const f = await fixture(t);
  assert.equal(f.focus(), 'primary');
  await f.key(39);
  assert.equal(f.focus(), 'receiver');
  for (const row of [
    'source',
    'compression',
    'autostart',
    'calibrate',
    'metrics',
  ]) {
    await f.key(40);
    assert.equal(f.focus(), row);
  }
  await f.key(39);
  await f.key(40); // Unchanged settings have no enabled Save action.
  assert.equal(f.focus(), 'metrics');
  assert.equal(f.w.document.querySelectorAll('select').length, 0);
  assert.equal(f.el('source-value').textContent, 'Video + TV menus');
  assert.equal(f.el('compression-value').textContent, 'LZ4');
  assert.equal(f.el('autostart-value').textContent, 'Off');
  assert.deepEqual(f.mutations(), []);
});

test('OK opens choices, arrows only focus, Back cancels, and Save alone persists', async (t) => {
  const f = await fixture(t);
  f.el('source').focus();
  await f.click('source');
  assert.equal(f.focus(), 'choice-both');
  assert.equal(f.el('choice-ui'), null);
  await f.key(40);
  assert.equal(f.focus(), 'choice-video');
  assert.equal(f.el('choice-both').getAttribute('aria-pressed'), 'true');
  assert.equal(f.value.config.source, 'both');
  await f.key(461);
  assert.equal(f.el('modal'), null);
  assert.equal(f.focus(), 'source');
  assert.equal(f.el('source-value').textContent, 'Video + TV menus');
  await f.click('source');
  await f.click('choice-video');
  assert.equal(f.el('source-value').textContent, 'Video only');
  assert.equal(f.el('saved-state').textContent, 'Unsaved changes');
  assert.equal(f.value.config.source, 'both');
  assert.deepEqual(f.mutations(), []);
  await f.click('save');
  assert.equal(f.value.config.source, 'video');
  assert.equal(f.mutations().length, 1);
  assert.equal(f.mutations()[0].method, 'setConfig');
  assert.equal(f.el('save').disabled, true);
});

test('receiver editing is explicit and cancels without overwriting the address', async (t) => {
  const f = await fixture(t);
  f.el('receiver').focus();
  await f.click('receiver');
  assert.equal(f.focus(), 'receiver-input');
  await f.input('receiver-input', 'https://prysm.test');
  await f.key(461);
  assert.equal(f.el('receiver-value').textContent, 'http://192.168.18.18:8787');
  await f.click('receiver');
  await f.input('receiver-input', 'https://prysm.test');
  await f.key(39);
  assert.equal(
    f.focus(),
    'receiver-input',
    'left/right remain cursor keys inside the editor',
  );
  await f.key(40);
  assert.equal(f.focus(), 'receiver-apply');
  await f.click('receiver-apply');
  assert.equal(f.el('receiver-value').textContent, 'https://prysm.test');
  assert.deepEqual(f.mutations(), []);
});

test('save-and-enable applies the draft before enabling, with no optimistic streaming claim', async (t) => {
  const f = await fixture(t);
  await f.click('compression');
  await f.click('choice-h264-software');
  assert.equal(f.el('primary').textContent, 'Save and enable');
  await f.click('primary');
  assert.deepEqual(
    f.mutations().map((call) => call.method),
    ['setConfig', 'start'],
  );
  assert.equal(f.value.config.compression, 'h264-software');
  assert.equal(f.el('status-title').textContent, 'Waiting for Prysm');
  assert.equal(f.el('primary').textContent, 'Disable');
  await f.click('primary');
  assert.equal(f.value.enabled, false);
  assert.equal(f.el('status-title').textContent, 'Disabled');
  assert.equal(f.el('notice').textContent, '');
});

test('Save completes when the service replies after the click', async (t) => {
  const f = await fixture(t, { deferMethods: ['setConfig'] });
  await f.click('metrics');
  await f.click('save');
  const [save] = f.mutations();
  await f.reply(
    f.pending[0],
    JSON.stringify({ ...f.value, config: save.payload }),
  );
  assert.equal(f.el('saved-state').textContent, 'Saved');
  assert.equal(f.el('notice').textContent, 'Settings saved.');
  assert.deepEqual(
    f.mutations().map(({ method }) => method),
    ['setConfig'],
  );
});

test('cleanup is tucked away and requires explicit confirmation; Back returns one level', async (t) => {
  const f = await fixture(t);
  f.el('service-options').focus();
  await f.click('service-options');
  await f.click('diagnostics');
  await f.key(461);
  assert.equal(f.el('dialog-title').textContent, 'Service options');
  await f.click('cleanup');
  assert.equal(f.focus(), 'keep-setup');
  await f.key(40);
  assert.equal(f.focus(), 'confirm-cleanup');
  assert.deepEqual(f.mutations(), []);
  await f.key(461);
  assert.equal(f.el('dialog-title').textContent, 'Service options');
  await f.click('cleanup');
  await f.click('confirm-cleanup');
  assert.equal(f.mutations().length, 1);
  assert.equal(f.mutations()[0].method, 'cleanup');
  assert.equal(f.el('modal'), null);
});

test('status subscription preserves an unsaved choice and its focus', async (t) => {
  const f = await fixture(t);
  f.el('autostart').focus();
  await f.click('autostart');
  await f.click('choice-true');
  assert.equal(f.focus(), 'autostart');
  await f.notify();
  assert.equal(f.focus(), 'autostart');
  assert.equal(f.el('autostart-value').textContent, 'On');
  assert.equal(f.value.config.autostart, false);
  assert.equal(f.el('saved-state').textContent, 'Unsaved changes');
});

test('a subscription alone updates the calibration origin and preserves unrelated edits', async (t) => {
  const f = await fixture(t, { deferMethods: ['setConfig'] });
  await f.click('receiver');
  await f.input('receiver-input', 'https://updated.example');
  await f.click('receiver-apply');
  await f.click('save');
  const [save] = f.mutations();
  await f.advance(30000); // The service saved the config, but its direct reply was lost.
  await f.click('metrics');
  f.value.config = save.payload;
  await f.notify();
  assert.equal(f.el('metrics').getAttribute('aria-checked'), 'true');
  assert.equal(f.el('saved-state').textContent, 'Unsaved changes');
  await f.click('calibrate');
  assert.equal(
    f.w.document.querySelector('iframe').src,
    'https://updated.example/calibration-tv.html',
  );
});

test('a late action reply cannot revert a newer subscription, and a restarted service can reset its revision', async (t) => {
  const f = await fixture(t, { deferMethods: ['start'] });
  await f.click('primary');
  const reply = JSON.stringify(f.value);
  f.value.revision++;
  f.value.config = { ...f.value.config, instanceUrl: 'https://second.example' };
  await f.notify();
  await f.reply(f.pending[0], reply);
  assert.equal(f.el('receiver-value').textContent, 'https://second.example');

  await f.click('primary');
  const previousProcessReply = JSON.stringify(f.value);
  f.value.pid++;
  f.value.revision = 0;
  f.value.config = {
    ...f.value.config,
    instanceUrl: 'https://restarted.example',
  };
  await f.notify();
  await f.reply(f.pending[1], previousProcessReply);
  assert.equal(f.el('receiver-value').textContent, 'https://restarted.example');
});

test('Back on the main screen asks before closing; modal Tab stays inside', async (t) => {
  const f = await fixture(t);
  await f.key(461);
  assert.equal(f.el('dialog-title').textContent, 'Close Prysm Capture?');
  assert.equal(f.focus(), 'stay');
  f.el('dialog-back').focus();
  await f.key(9);
  assert.equal(f.focus(), 'stay');
  await f.key(27);
  assert.equal(f.el('modal'), null);
  assert.equal(f.focus(), 'primary');
});

test('unavailable service offers setup and keeps the error visible', async (t) => {
  const f = await fixture(t, { offline: true });
  assert.equal(f.el('primary').textContent, 'Set up service');
  assert.equal(f.el('notice').getAttribute('role'), 'alert');
  assert.equal(f.el('save').disabled, true);
});

test('a waiting sender is described as enabled, rather than actively capturing or paused', async (t) => {
  const f = await fixture(t);
  assert.equal(f.el('primary').textContent, 'Enable');
  await f.click('primary');
  f.value.lastLog = 'capture handshake failed: Connection refused';
  await f.notify();
  assert.equal(f.el('primary').textContent, 'Disable');
  assert.equal(f.el('connection').textContent, 'Waiting for Prysm');
  assert.equal(f.el('status-title').textContent, 'Waiting for Prysm');
  assert.match(
    f.el('status-description').textContent,
    /reconnect to your saved instance/,
  );
  assert.equal(
    f.value.enabled,
    true,
    'pipeline stop must leave the sender available for the next start',
  );
  f.value.receiverConnected = true;
  await f.notify();
  assert.equal(f.el('connection').textContent, 'Ready to capture');
  assert.equal(f.el('status-title').textContent, 'Ready to capture');
  f.value.capturing = true;
  await f.notify();
  assert.equal(f.el('status-title').textContent, 'Capturing');
  f.value.receiverConnected = false;
  await f.notify();
  assert.equal(f.el('connection').textContent, 'Waiting for Prysm');
});

test('one status subscription recovers after service failure without losing draft settings', async (t) => {
  const f = await fixture(t);
  assert.deepEqual(
    f.calls
      .filter((call) => call.method === 'status')
      .map((call) => call.payload),
    [{ subscribe: true }],
  );
  await f.click('compression');
  await f.click('choice-h264-software');
  const old = f.subscription();
  await f.reply(old, '{"returnValue":false,"errorText":"service exited"}');
  assert.equal(old.cancelled, true);
  assert.equal(f.el('connection').textContent, 'TV service unavailable');
  await f.advance(2999);
  assert.equal(f.calls.length, 1);
  await f.advance(1);
  assert.notEqual(f.subscription(), old);
  assert.equal(f.el('compression-value').textContent, 'H.264 (software)');
  assert.equal(f.el('saved-state').textContent, 'Unsaved changes');
  await f.reply(old, '{"returnValue":false,"errorText":"late reply"}');
  assert.notEqual(f.el('connection').textContent, 'TV service unavailable');
});

test('a silent activation times out; hiding the page cancels retries and synchronous cancel callbacks', async (t) => {
  const f = await fixture(t, { silentStatus: true, cancelReentry: true });
  await f.advance(30000);
  assert.equal(f.el('connection').textContent, 'TV service unavailable');
  await f.event('pagehide');
  const calls = f.calls.length;
  await f.advance(60000);
  assert.equal(f.calls.length, calls);
  f.options.silentStatus = false;
  await f.event('pageshow');
  assert.equal(f.el('connection').textContent, 'Disabled');
});

test('switching apps pauses sender updates until Prysm is visible again', async (t) => {
  const f = await fixture(t, { cancelReentry: true });
  const previous = f.subscription();
  async function setHidden(hidden) {
    Object.defineProperty(f.w.document, 'hidden', {
      value: hidden,
      configurable: true,
    });
    await f.settle(() =>
      f.w.document.dispatchEvent(new f.w.Event('visibilitychange')),
    );
  }
  await setHidden(true);
  assert.equal(previous.cancelled, true);
  await f.reply(previous, '{"returnValue":false}');
  await f.advance(30000);
  assert.equal(f.calls.length, 1);
  await setHidden(false);
  assert.notEqual(f.subscription(), previous);
  assert.equal(f.el('connection').textContent, 'Disabled');
});

test('malformed or unsubscribed status replies are cancelled and retried', async (t) => {
  const f = await fixture(t);
  await f.reply(f.subscription(), '{');
  assert.equal(f.el('connection').textContent, 'TV service unavailable');
  await f.advance(3000);
  await f.reply(
    f.subscription(),
    JSON.stringify({ ...f.value, subscribed: false }),
  );
  assert.equal(f.el('connection').textContent, 'TV service unavailable');
  await f.advance(3000);
  assert.equal(f.el('connection').textContent, 'Disabled');
});

test('subscriptions and actions reject replies that violate the service contract', async (t) => {
  const f = await fixture(t, { deferMethods: ['start'] });
  const invalid = {
    ...f.value,
    config: { ...f.value.config, compression: 'zip' },
  };
  await f.reply(
    f.subscription(),
    JSON.stringify({ ...invalid, subscribed: true }),
  );
  assert.equal(f.el('connection').textContent, 'TV service unavailable');
  await f.advance(3000);
  await f.reply(
    f.subscription(),
    JSON.stringify({
      ...f.value,
      subscribed: true,
      revision: Number.MAX_SAFE_INTEGER,
    }),
  );
  assert.equal(f.el('connection').textContent, 'Disabled');
  for (const reply of [
    invalid,
    { ...f.value, capabilities: {} },
    { ...f.value, capabilities: { hardwareH264: 'true' } },
    {
      ...f.value,
      capabilities: { hardwareH264: false, embeddedPortal: 'true' },
    },
    { ...f.value, portalUrl: 8787 },
    { ...f.value, receiverBusy: 'false' },
    { ...f.value, subscribers: 2 ** 32 },
    { ...f.value, revision: Number.MAX_SAFE_INTEGER + 1 },
    { ...f.value, config: { ...f.value.config, unknown: true } },
    { returnValue: false, errorText: 42 },
  ]) {
    await f.click('primary');
    await f.reply(f.pending.at(-1), JSON.stringify(reply));
    assert.equal(
      f.el('notice').textContent,
      'The service returned an invalid reply.',
    );
    assert.equal(f.el('primary').disabled, false);
  }
});

test('Homebrew setup accepts exec success and reports stderr without a Prysm status', async (t) => {
  const f = await fixture(t, { offline: true, deferMethods: ['exec'] });
  await f.click('primary');
  await f.reply(f.pending.at(-1), JSON.stringify({ stdoutString: 'done' }));
  assert.equal(
    f.el('notice').textContent,
    'Homebrew Channel returned an invalid reply.',
  );
  await f.click('primary');
  await f.reply(
    f.pending.at(-1),
    JSON.stringify({ returnValue: false, stderrString: 'Setup script failed' }),
  );
  assert.equal(f.el('notice').textContent, 'Setup script failed');
  await f.click('primary');
  f.options.offline = false;
  await f.reply(
    f.pending.at(-1),
    JSON.stringify({ returnValue: true, stderrString: 'Setup warning' }),
  );
  assert.equal(f.el('connection').textContent, 'Disabled');
});

test('metrics checkbox defaults off and only persists on Save', async (t) => {
  const f = await fixture(t);
  const checkbox = f.el('metrics');
  assert.equal(checkbox.getAttribute('role'), 'checkbox');
  assert.equal(checkbox.getAttribute('aria-checked'), 'false');
  checkbox.focus();
  await f.click('metrics');
  assert.equal(f.focus(), 'metrics');
  assert.equal(checkbox.getAttribute('aria-checked'), 'true');
  assert.deepEqual(f.mutations(), []);
  await f.notify();
  assert.equal(checkbox.getAttribute('aria-checked'), 'true');
  await f.key(40);
  assert.equal(f.focus(), 'save');
  await f.click('save');
  assert.equal(f.value.config.metrics, true);
  assert.equal(f.mutations()[0].payload.metrics, true);
  await f.click('metrics');
  await f.click('save');
  assert.equal(f.value.config.metrics, false);
  assert.equal(checkbox.getAttribute('aria-checked'), 'false');
});

for (const [compression, label] of [
  ['h264-software', 'H.264 (software)'],
  ['h264-hardware', 'H.264 (hardware)'],
]) {
  test(`${label} is saved and restored as a distinct compression choice`, async (t) => {
    const f = await fixture(t, { hardwareH264: true });
    await f.click('compression');
    await f.click('choice-' + compression);
    assert.equal(f.el('compression-value').textContent, label);
    assert.equal(f.value.config.compression, 'lz4');
    await f.click('save');
    assert.equal(f.value.config.compression, compression);
    assert.equal(f.mutations()[0].payload.compression, compression);
    await f.notify();
    await f.click('compression');
    assert.equal(
      f.el('choice-' + compression).getAttribute('aria-pressed'),
      'true',
    );
    assert.equal(f.focus(), 'choice-' + compression);
  });
}

test('hardware compression is offered only when the service advertises support', async (t) => {
  const f = await fixture(t);
  await f.click('compression');
  assert.equal(f.el('choice-h264-hardware'), null);
  assert.ok(f.el('choice-h264-software'));
  await f.key(461);
  f.value.capabilities = { hardwareH264: true };
  await f.notify();
  await f.click('compression');
  assert.ok(f.el('choice-h264-hardware'));
});

test('TV output uses relative classic scripts and excludes the local service simulation', () => {
  assert.doesNotMatch(html, /type="module"|modulepreload|crossorigin/);
  const scripts = [...html.matchAll(/<script[^>]+src="([^"]+)"/g)];
  assert.equal(scripts.length, 1);
  assert.ok(scripts[0][1].startsWith('./'));
  assert.match(html, /<script defer /);
  assert.doesNotMatch(
    script,
    /Local development: simulated TV service|Simulated TV|import\.meta/,
  );
  for (const name of ['app.css', 'appinfo.json', 'icon.png']) {
    assert.ok(fs.existsSync(path.join(dist, name)));
  }
  assert.equal(
    JSON.parse(fs.readFileSync(path.join(dist, 'appinfo.json'))).main,
    'index.html',
  );
});

test('local development service supports settings, subscriptions, start, stop and cleanup', async (t) => {
  const f = await fixture(t, { devService: true });
  assert.equal(f.el('status-title').textContent, 'Disabled');
  await f.click('autostart');
  await f.click('choice-true');
  await f.click('primary');
  assert.equal(f.el('saved-state').textContent, 'Saved');
  assert.equal(f.el('status-title').textContent, 'Capturing');
  await f.click('primary');
  assert.equal(f.el('status-title').textContent, 'Disabled');
  await f.click('service-options');
  await f.click('diagnostics');
  assert.match(
    f.el('dialog-body').textContent,
    /Local development: simulated TV service/,
  );
  await f.key(27);
  await f.click('cleanup');
  await f.click('confirm-cleanup');
  assert.equal(f.el('autostart-value').textContent, 'Off');
  await f.event('pagehide');
  await f.event('pageshow');
  assert.equal(f.el('status-title').textContent, 'Disabled');
});

test('receiver input survives status updates, trims on apply, and resets after cancelling', async (t) => {
  const f = await fixture(t);
  await f.click('receiver');
  await f.input('receiver-input', '  https://prysm.test  ');
  await f.notify();
  assert.equal(f.el('receiver-input').value, '  https://prysm.test  ');
  assert.equal(f.focus(), 'receiver-input');
  await f.click('receiver-apply');
  assert.equal(f.el('receiver-value').textContent, 'https://prysm.test');
  await f.click('receiver');
  await f.input('receiver-input', 'discard me');
  await f.key(461);
  await f.click('receiver');
  assert.equal(f.el('receiver-input').value, 'https://prysm.test');
  assert.deepEqual(f.mutations(), []);
});

test('failed save keeps the draft and never starts the sender', async (t) => {
  const f = await fixture(t, { deferMethods: ['setConfig'] });
  await f.click('metrics');
  await f.click('primary');
  assert.equal(f.el('primary').disabled, true);
  await f.reply(
    f.pending[0],
    '{"returnValue":false,"errorText":"Save failed"}',
  );
  assert.equal(f.el('notice').textContent, 'Save failed');
  assert.equal(f.el('primary').disabled, false);
  assert.equal(f.el('saved-state').textContent, 'Unsaved changes');
  assert.equal(f.el('metrics').getAttribute('aria-checked'), 'true');
  assert.deepEqual(
    f.mutations().map((call) => call.method),
    ['setConfig'],
  );
});

test('request timeout ignores cancel reentry and late replies, then permits retry', async (t) => {
  const f = await fixture(t, { deferMethods: ['start'], cancelReentry: true });
  await f.click('primary');
  await f.advance(30000);
  assert.equal(f.pending[0].cancelled, true);
  assert.match(f.el('notice').textContent, /did not respond/);
  assert.equal(f.el('primary').disabled, false);
  await f.reply(f.pending[0], JSON.stringify({ ...f.value, enabled: true }));
  assert.equal(f.el('primary').textContent, 'Enable');
  f.options.deferMethods = [];
  await f.click('primary');
  assert.equal(f.el('primary').textContent, 'Disable');
});

test('synchronous bridge errors and invalid action replies release the busy state', async (t) => {
  const f = await fixture(t, { throwMethod: 'start' });
  await f.click('primary');
  assert.match(f.el('notice').textContent, /Bridge call failed/);
  assert.equal(f.el('primary').disabled, false);
  f.options.throwMethod = undefined;
  f.options.deferMethods = ['start'];
  for (const reply of [
    'null',
    '{',
    '[]',
    '{"returnValue":true,"config":{"receiver":"old"}}',
  ]) {
    await f.click('primary');
    await f.reply(f.pending[f.pending.length - 1], reply);
    assert.equal(f.el('notice').getAttribute('role'), 'alert');
    assert.equal(f.el('primary').disabled, false);
  }
  f.options.deferMethods = [];
  await f.click('primary');
  assert.equal(f.el('primary').textContent, 'Disable');
});

test('service clients own cancellation independently and can be reused after effect cleanup', async (t) => {
  const f = await fixture(t, { deferMethods: ['start'], cancelReentry: true });
  const exports = {};
  f.w.Function(
    'exports',
    'require',
    service,
  )(exports, createRequire(path.join(__dirname, '../src/service.ts')));
  const first = exports.createServiceClient();
  const second = exports.createServiceClient();
  const replies = [];
  const uri = 'luna://io.prysm.capture.service/start';
  first.call(uri, {}, exports.parseReply, (value) => replies.push(value));
  second.call(uri, {}, exports.parseReply, (value) => replies.push(value));
  first.cancelAll();
  assert.equal(f.pending[0].cancelled, true);
  assert.equal(f.pending[1].cancelled, undefined);
  await f.reply(f.pending[0], JSON.stringify(f.value));
  assert.equal(replies.length, 0);
  await f.reply(f.pending[1], JSON.stringify(f.value));
  await f.reply(f.pending[1], JSON.stringify(f.value));
  assert.equal(replies.length, 1, 'each request completes only once');
  await f.advance(30000);
  assert.equal(replies.length, 1, 'cleanup removes request timeouts');
  first.call(uri, {}, exports.parseReply, (value) => replies.push(value));
  await f.reply(f.pending[2], JSON.stringify(f.value));
  assert.equal(replies.length, 2);
});

test('status has one live location and setting explanations remain secondary text', async (t) => {
  const f = await fixture(t);
  assert.equal(f.w.document.querySelector('header #connection'), null);
  assert.equal(f.el('connection').getAttribute('role'), 'status');
  assert.equal(f.el('connection').getAttribute('aria-live'), 'polite');
  assert.equal(
    f.el('connection').querySelector('#status-title'),
    f.el('status-title'),
  );
  assert.equal(f.el('metrics-description').className, 'setting-detail');
  await f.click('primary');
  f.value.receiverConnected = true;
  await f.notify();
  assert.equal(f.el('status-title').textContent, 'Ready to capture');
  assert.match(
    f.el('status-description').textContent,
    /Start capture in Prysm/,
  );
  f.value.power = 'Standby';
  await f.notify();
  assert.equal(f.el('status-title').textContent, 'Waiting for TV');
  f.value.stopping = true;
  await f.notify();
  assert.equal(f.el('status-title').textContent, 'Stopping capture…');
  f.value.error = 'capture failed';
  await f.notify();
  assert.equal(f.el('status-title').textContent, 'Capture needs attention');
  assert.ok(f.el('status-title').closest('.attention'));
  assert.match(f.el('status-description').textContent, /diagnostics or repair/);
});

test('no video keeps an enabled sender in a stable waiting state across receiver retries', async (t) => {
  const f = await fixture(t);
  await f.click('primary');
  f.value.videoAvailable = false;
  f.value.receiverConnected = true;
  f.value.capturing = true;
  await f.notify();
  assert.equal(f.el('status-title').textContent, 'Waiting for video');
  assert.match(
    f.el('status-description').textContent,
    /HDMI input or play a video/,
  );
  assert.equal(f.el('primary').textContent, 'Disable');
  assert.equal(f.w.document.querySelector('.ready'), null);
  f.value.receiverConnected = false;
  f.value.capturing = false;
  await f.notify();
  assert.equal(f.el('status-title').textContent, 'Waiting for video');
  f.value.stopping = true;
  await f.notify();
  assert.equal(f.el('status-title').textContent, 'Waiting for video');
  f.value.stopping = false;
  assert.equal(f.value.enabled, true);
  f.value.videoAvailable = true;
  await f.notify();
  assert.equal(f.el('status-title').textContent, 'Waiting for Prysm');
  f.value.receiverConnected = true;
  await f.notify();
  assert.equal(f.el('status-title').textContent, 'Ready to capture');
  assert.deepEqual(
    f.mutations().map((call) => call.method),
    ['start'],
  );
});

test('TV calibration pairs through the same-origin relay, hides QR during video and releases playback on Back', async (t) => {
  const f = await fixture(t);
  let heartbeat;
  f.w.setInterval = (poll) => {
    heartbeat = poll;
    return 42;
  };
  f.w.clearInterval = () => {};
  f.w.HTMLMediaElement.prototype.play = () => Promise.resolve();
  f.w.HTMLMediaElement.prototype.pause = () => {};
  f.w.HTMLMediaElement.prototype.load = () => {};
  await f.click('calibrate');
  const relay = f.w.document.querySelector('iframe');
  const messages = [];
  relay.contentWindow.postMessage = (message, origin) =>
    messages.push({ message, origin });
  const send = (data, origin = 'http://192.168.18.18:8787') =>
    f.settle(() =>
      f.w.dispatchEvent(
        new f.w.MessageEvent('message', {
          source: relay.contentWindow,
          origin,
          data,
        }),
      ),
    );
  await send(
    { type: 'calibration-error', message: 'untrusted' },
    'http://attacker.invalid',
  );
  assert.equal(f.w.document.querySelector('[role=alert]'), null);
  heartbeat();
  assert.deepEqual(
    { ...messages[0].message },
    { action: 'poll', playing: false },
  );
  let view = {
    token: 'tv',
    pairing: 'pair',
    connected: false,
    draft: {
      id: 'profile',
      name: 'Calibration 1',
      correction: { warmth: 0, tint: 0, midtones: 0 },
    },
    step: 'choose',
    shade: 0,
    paused: false,
    compare: false,
    sequence: 0,
    ended: null,
  };
  await send({ type: 'calibration', view });
  assert.ok(
    f.w.document.querySelector(
      'img[alt="Scan to open the calibration wizard"]',
    ),
  );
  view = { ...view, connected: true, pairing: null };
  await send({ type: 'calibration', view });
  assert.equal(f.w.document.querySelector('img'), null);
  assert.match(f.w.document.body.textContent, /Calibration in progress/);
  assert.equal(f.w.document.querySelector('video'), null);
  assert.equal(f.focus(), 'calibration-exit');
  await f.settle(() => f.advance(20000));
  assert.match(f.w.document.body.textContent, /Calibration in progress/);
  assert.ok(!messages.some(({ message }) => message.action === 'cancel'));
  view = { ...view, step: 'response' };
  await send({ type: 'calibration', view });
  assert.equal(
    f.w.document.querySelector('video').getAttribute('src'),
    'calibration/response.mp4',
  );
  const response = f.w.document.querySelector('video');
  Object.defineProperties(response, {
    paused: { value: false },
    currentTime: { value: 0.5 },
  });
  heartbeat();
  assert.equal(messages.at(-1).message.playing, true);
  await send({
    type: 'calibration',
    view: { ...view, step: 'choose', startup_pending: true },
  });
  assert.equal(f.w.document.querySelector('video'), response);
  assert.equal(response.getAttribute('src'), 'calibration/response.mp4');
  await send({ type: 'calibration', view: { ...view, step: 'choose' } });
  assert.equal(f.w.document.querySelector('video'), null);
  assert.equal(response.hasAttribute('src'), false);
  view = { ...view, step: 'white_balance' };
  await send({ type: 'calibration', view });
  const video = f.w.document.querySelector('video');
  assert.equal(video.getAttribute('src'), 'calibration/gray-192.mp4');
  f.w.HTMLMediaElement.prototype.play = () =>
    Promise.reject(
      new f.w.DOMException('Playback intentionally paused', 'AbortError'),
    );
  view = { ...view, step: 'colors', paused: true };
  await send({ type: 'calibration', view });
  assert.ok(
    f.w.document.querySelector('video'),
    'an intentional Pause must not exit calibration',
  );
  for (const [shade, color] of [
    'red',
    'green',
    'blue-pair',
    'cyan',
    'magenta',
    'yellow',
  ].entries()) {
    view = { ...view, shade, paused: true };
    await send({ type: 'calibration', view });
    assert.equal(
      f.w.document.querySelector('video').getAttribute('src'),
      `calibration/${color}.mp4`,
    );
  }
  const pending = () => f.w.document.querySelector('.pattern-pending');
  await f.settle(() => video.dispatchEvent(new f.w.Event('playing')));
  assert.equal(pending(), null);
  await send({ type: 'calibration', view: { ...view, shade: 0 } });
  await send({ type: 'calibration', view });
  assert.ok(pending(), 'returning to the last color still reloads its MP4');
  await f.settle(() => video.dispatchEvent(new f.w.Event('playing')));
  assert.equal(pending(), null);
  view = { ...view, paused: false };
  await send({ type: 'calibration', view });
  assert.equal(
    f.w.document.querySelector('video').getAttribute('src'),
    'calibration/colors.mp4',
  );
  for (const [step, clips] of [
    ['intensity', ['blue-pair', 'red-pair', 'green-pair']],
    ['mixed', ['coast', 'garden', 'dusk']],
  ]) {
    for (const [shade, clip] of clips.entries()) {
      view = { ...view, step, shade };
      await send({ type: 'calibration', view });
      assert.equal(
        f.w.document.querySelector('video').getAttribute('src'),
        'calibration/' + clip + '.mp4',
      );
    }
  }
  await f.key(461);
  assert.equal(f.w.document.querySelector('video'), null);
  assert.equal(video.hasAttribute('src'), false);
  assert.equal(messages.at(-1).message.action, 'cancel');
  await send({ type: 'calibration', view });
  assert.equal(f.w.document.querySelector('video'), null);
});

for (const saved of [true, false]) {
  test(`ending review shows the ${saved ? 'saved' : 'discarded'} outcome and Return exits calibration`, async (t) => {
    const f = await fixture(t);
    f.w.setInterval = () => 42;
    f.w.HTMLMediaElement.prototype.play = () => Promise.resolve();
    f.w.HTMLMediaElement.prototype.pause = () => {};
    f.w.HTMLMediaElement.prototype.load = () => {};
    await f.click('calibrate');
    const relay = f.w.document.querySelector('iframe').contentWindow;
    relay.postMessage = () => {};
    const review = { token: 'tv', connected: true, step: 'review', shade: 0 };
    const show = (view) =>
      f.settle(() =>
        f.w.dispatchEvent(
          new f.w.MessageEvent('message', {
            source: relay,
            origin: 'http://192.168.18.18:8787',
            data: { type: 'calibration', view },
          }),
        ),
      );
    await show(review);
    const video = f.w.document.querySelector('video');
    assert.equal(video.getAttribute('src'), 'calibration/review-grid.mp4');
    const ended = saved
      ? 'Calibration saved and active.'
      : 'Calibration cancelled.';
    await show({ ...review, saved, ended });
    await f.event('pagehide');
    assert.equal(f.w.document.querySelector('video'), null);
    assert.equal(video.hasAttribute('src'), false);
    assert.equal(
      f.w.document.querySelector('.pairing-copy h2').textContent,
      saved ? 'Calibration finished' : 'Calibration ended',
    );
    assert.equal(
      f.w.document.querySelector('.pairing-lead').textContent,
      ended,
    );
    assert.match(
      f.w.document.querySelector('.pairing-status').textContent,
      /Press Return/,
    );
    assert.equal(f.focus(), 'calibration-exit');
    await f.key(461);
    assert.equal(f.w.document.querySelector('.calibration-screen'), null);
    assert.equal(f.focus(), 'calibrate');
  });
}

for (const event of ['pagehide', 'visibilitychange']) {
  test(`TV playback releases the decoder immediately and only once on ${event}`, async (t) => {
    const f = await fixture(t);
    f.w.setInterval = () => 42;
    f.w.HTMLMediaElement.prototype.play = () => Promise.resolve();
    let pauses = 0;
    let loads = 0;
    f.w.HTMLMediaElement.prototype.pause = () => {
      pauses++;
    };
    f.w.HTMLMediaElement.prototype.load = () => {
      loads++;
    };
    await f.click('calibrate');
    const relay = f.w.document.querySelector('iframe').contentWindow;
    const messages = [];
    relay.postMessage = (message) => messages.push(message);
    await f.settle(() =>
      f.w.dispatchEvent(
        new f.w.MessageEvent('message', {
          source: relay,
          origin: 'http://192.168.18.18:8787',
          data: {
            type: 'calibration',
            view: { token: 'tv', step: 'response', shade: 0, connected: true },
          },
        }),
      ),
    );
    const video = f.w.document.querySelector('video');
    assert.ok(video.hasAttribute('src'));
    await f.settle(() => {
      if (event === 'pagehide') f.w.dispatchEvent(new f.w.Event(event));
      else {
        Object.defineProperty(f.w.document, 'hidden', {
          value: true,
          configurable: true,
        });
        f.w.document.dispatchEvent(new f.w.Event(event));
      }
      assert.equal(video.hasAttribute('src'), false);
    });
    assert.ok(messages.some(({ action }) => action === 'cancel'));
    await f.key(461);
    assert.deepEqual([pauses, loads], [1, 1]);
  });
}

test('the initial connection timeout preserves the reason pairing failed', async (t) => {
  const f = await fixture(t);
  f.w.setInterval = () => 42;
  await f.click('calibrate');
  const message =
    'Calibration in progress—finish or exit on the TV to change settings.';
  await f.settle(() =>
    f.w.dispatchEvent(
      new f.w.MessageEvent('message', {
        source: f.w.document.querySelector('iframe').contentWindow,
        origin: 'http://192.168.18.18:8787',
        data: { type: 'calibration-error', terminal: false, message },
      }),
    ),
  );
  await f.advance(16000);
  assert.equal(f.w.document.querySelector('[role=alert]').textContent, message);
});
test('a terminal playback error stays visible past the reconnect deadline', async (t) => {
  const f = await fixture(t);
  f.w.setInterval = () => 42;
  f.w.HTMLMediaElement.prototype.play = () =>
    Promise.reject(new Error('Decode failed'));
  f.w.HTMLMediaElement.prototype.pause = () => {};
  f.w.HTMLMediaElement.prototype.load = () => {};
  await f.click('calibrate');
  const relay = f.w.document.querySelector('iframe').contentWindow;
  await f.settle(() =>
    f.w.dispatchEvent(
      new f.w.MessageEvent('message', {
        source: relay,
        origin: 'http://192.168.18.18:8787',
        data: {
          type: 'calibration',
          view: { token: 'tv', step: 'response', shade: 0, connected: true },
        },
      }),
    ),
  );
  const alert = () => f.w.document.querySelector('[role=alert]').textContent;
  assert.equal(alert(), 'Video failed. Exit and try again.');
  await f.advance(31000);
  assert.equal(alert(), 'Video failed. Exit and try again.');
  assert.equal(f.w.document.querySelector('video'), null);
});

for (const restarted of [true, false]) {
  test(`Save and enable ignores a reply superseded by a ${restarted ? 'restarted service' : 'newer configuration'}`, async (t) => {
    const f = await fixture(t, { deferMethods: ['setConfig'] });
    await f.click('metrics');
    await f.click('primary');
    const reply = { ...f.value, config: f.mutations()[0].payload };
    if (restarted) f.value.pid++;
    f.value.revision++;
    f.value.config = {
      ...f.value.config,
      instanceUrl: 'https://other.example',
    };
    await f.notify();
    await f.reply(f.pending[0], JSON.stringify(reply));
    assert.deepEqual(
      f.mutations().map(({ method }) => method),
      ['setConfig'],
    );
    assert.notEqual(f.el('notice').textContent, 'Settings saved.');
    assert.equal(f.el('saved-state').textContent, 'Unsaved changes');
  });
}

test('a failed save leaves the entered URL and uses current settings on the next save', async (t) => {
  const f = await fixture(t, { deferMethods: ['setConfig'] });
  await f.click('receiver');
  await f.input('receiver-input', 'http://prysm.test');
  await f.click('receiver-apply');
  await f.click('primary');
  await f.reply(
    f.pending[0],
    JSON.stringify({
      returnValue: false,
      errorText: 'Connection refused',
    }),
  );
  assert.equal(f.el('notice').textContent, 'Connection refused');
  assert.equal(f.el('save').textContent, 'Save changes');
  f.value.config.metrics = true;
  f.value.revision++;
  await f.notify();
  await f.click('save');
  assert.deepEqual(
    f.mutations().map(({ method }) => method),
    ['setConfig', 'setConfig'],
  );
  assert.deepEqual(f.mutations()[1].payload, {
    ...f.value.config,
    instanceUrl: 'http://prysm.test',
  });
});

test('the calibration relay uses the saved HTTPS origin', async (t) => {
  const f = await fixture(t, { instanceUrl: 'https://prysm.example' });
  f.w.setInterval = () => 42;
  await f.click('calibrate');
  assert.equal(
    f.w.document.querySelector('iframe').src,
    'https://prysm.example/calibration-tv.html',
  );
  assert.equal(f.el('calibration-address'), null);
});

test('the instance editor displays the resolved LAN address before Save', async (t) => {
  const f = await fixture(t);
  for (const [input, expected] of [
    ['192.168.1.10', 'http://192.168.1.10:8787'],
    ['[::1]', 'http://[::1]:8787'],
    ['prysm.local:80', 'http://prysm.local:80'],
    ['https://prysm.example', 'https://prysm.example'],
  ]) {
    await f.click('receiver');
    await f.input('receiver-input', input);
    await f.click('receiver-apply');
    assert.equal(f.el('receiver-value').textContent, expected);
  }
});

const portalUrl = 'http://192.0.2.10:8787/';
test('processing location requires the portal build and matching service', async (t) => {
  const f = await fixture(t);
  assert.equal(f.el('location'), null);
  f.value.capabilities.embeddedPortal = true;
  await f.notify();
  assert.equal(!!f.el('location'), embeddedBuild);
  assert.equal(script.includes('On this TV (experimental)'), embeddedBuild);
  assert.equal(script.includes('Use this setup'), false);
  assert.equal(script.includes('Prysm web portal'), embeddedBuild);
});

test('a mismatched service with TV-owned lighting settings cannot be edited', async (t) => {
  const f = await fixture(t, { capabilities: { embeddedPrysm: true } });
  assert.equal(f.el('location'), null);
  assert.match(f.el('notice').textContent, /matching Prysm Capture package/);
  assert.deepEqual(f.mutations(), []);
});

test(
  'embedded selection needs no receiver or WLED and the portal remains visible while paused',
  { skip: !embeddedBuild },
  async (t) => {
    const f = await fixture(t, {
      config: { instanceUrl: null },
      capabilities: { embeddedPortal: true },
    });
    await f.key(39);
    await f.click('location');
    await f.click('choice-embedded');
    assert.equal(f.focus(), 'location');
    for (const id of [
      'receiver',
      'compression',
      'wled',
      'calibrate',
      'portal-address',
    ]) {
      assert.equal(f.el(id), null);
    }
    assert.equal(f.el('primary').textContent, 'Use this TV');
    await f.click('primary');
    assert.deepEqual(
      f.mutations().map(({ method }) => method),
      ['setConfig'],
    );
    assert.equal(f.value.config.instanceUrl, null);
    assert.equal(f.value.config.location, 'embedded');
    assert.equal('embeddedOutput' in f.value.config, false);
    f.value.enabled = true;
    f.value.senderRunning = true;
    f.value.portalUrl = portalUrl;
    await f.notify();
    assert.match(f.el('portal-address').textContent, /192\.0\.2\.10:8787/);
    assert.equal(f.el('status-title').textContent, 'Prysm is ready');
    await f.click('primary');
    assert.equal(f.mutations().at(-1).method, 'stop');
    assert.equal(f.el('status-title').textContent, 'TV capture paused');
    assert.equal(
      f.el('portal-address').querySelector('code').textContent,
      portalUrl,
    );
    delete f.value.portalUrl;
    f.value.error = 'cannot bind portal';
    await f.notify();
    assert.equal(f.el('portal-address'), null);
    assert.equal(f.el('calibrate'), null);
    assert.equal(f.el('status-title').textContent, 'Capture needs attention');
  },
);

test(
  'allow TV capture saves pending embedded edits before resuming',
  { skip: !embeddedBuild },
  async (t) => {
    const f = await fixture(t, {
      config: { location: 'embedded' },
      capabilities: { embeddedPortal: true },
    });
    await f.click('source');
    await f.click('choice-video');
    assert.equal(f.el('primary').textContent, 'Allow TV capture');
    await f.click('primary');
    assert.deepEqual(
      f.mutations().map(({ method }) => method),
      ['setConfig', 'start'],
    );
    assert.equal(f.value.config.source, 'video');
  },
);

test(
  'the embedded calibration relay uses the actual local portal origin',
  { skip: !embeddedBuild },
  async (t) => {
    const f = await fixture(t, {
      config: { location: 'embedded', instanceUrl: 'https://remote.example' },
      capabilities: { embeddedPortal: true },
    });
    f.value.portalUrl = portalUrl;
    await f.notify();
    f.w.setInterval = () => 42;
    await f.click('calibrate');
    assert.equal(
      f.w.document.querySelector('iframe').src,
      portalUrl + 'calibration-tv.html',
    );
  },
);

test(
  'mode changes preserve receiver preferences and describe the running mode until saved',
  { skip: !embeddedBuild },
  async (t) => {
    const f = await fixture(t, { capabilities: { embeddedPortal: true } });
    f.value.enabled = true;
    f.value.receiverConnected = true;
    f.value.capturing = true;
    await f.notify();
    await f.click('location');
    await f.click('choice-embedded');
    assert.match(f.el('status-description').textContent, /sent to Prysm/);
    await f.click('save');
    assert.equal(f.value.config.instanceUrl, 'http://192.168.18.18:8787');
    await f.click('location');
    await f.click('choice-remote');
    assert.ok(f.el('compression'));
    assert.equal(f.el('calibrate'), null);
    await f.click('save');
    assert.ok(f.el('calibrate'));
    assert.equal(f.el('portal-address'), null);
  },
);

test(
  'a standard UI cannot start embedded mode on a mismatched service',
  { skip: embeddedBuild },
  async (t) => {
    const f = await fixture(t, {
      capabilities: { embeddedPortal: true },
      config: { location: 'embedded' },
    });
    assert.equal(f.el('location'), null);
    await f.click('primary');
    assert.deepEqual(
      f.mutations().map(({ method }) => method),
      ['setConfig', 'start'],
    );
    assert.equal(f.value.config.location, 'remote');
  },
);

test(
  'a newer embedded selection prevents a stale remote Save from enabling another target',
  { skip: !embeddedBuild },
  async (t) => {
    const f = await fixture(t, {
      capabilities: { embeddedPortal: true },
      deferMethods: ['setConfig'],
    });
    await f.click('metrics');
    await f.click('primary');
    const oldRemote = f.mutations()[0].payload;
    assert.equal(oldRemote.location, undefined);
    const reply = JSON.stringify({ ...f.value, config: oldRemote });
    f.value.config = { ...oldRemote, location: 'embedded' };
    f.value.revision++;
    await f.notify();
    await f.reply(f.pending[0], reply);
    assert.deepEqual(
      f.mutations().map(({ method }) => method),
      ['setConfig'],
    );
    assert.equal(
      f.el('location-value').textContent,
      'On this TV (experimental)',
    );
  },
);
