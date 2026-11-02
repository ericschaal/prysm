'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('jsdom');
const ts = require('typescript');
const devService = ts.transpileModule(fs.readFileSync(path.join(__dirname, '../src/dev-service.ts'), 'utf8'), {
    compilerOptions: { target: ts.ScriptTarget.ES2015, module: ts.ModuleKind.CommonJS }
}).outputText;
const html = fs.readFileSync(path.join(__dirname, '../dist/index.html'), 'utf8');
const script = fs.readFileSync(path.join(__dirname, '../dist/' + html.match(/<script[^>]+src="([^"]+)"/)[1]), 'utf8');

async function fixture(t, options = {}) {
    const dom = new JSDOM(html, { runScripts: 'outside-only', url: 'file:///media/developer/apps/usr/palm/applications/io.prysm.capture/index.html' });
    t.after(() => dom.window.close());
    const w = dom.window;
    const tasks = [];
    w.setImmediate = callback => { tasks.push(callback); };
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
    w.setTimeout = (callback, delay) => { const id = ++timerId; timers.set(id, { at: now + delay, callback }); return id; };
    w.clearTimeout = id => timers.delete(id);
    w.setInterval = () => { throw new Error('status must use a subscription'); };
    function advance(milliseconds) {
        const end = now + milliseconds;
        while (true) {
            const next = [...timers.entries()].filter(([, timer]) => timer.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
            if (!next) break;
            timers.delete(next[0]); now = next[1].at; next[1].callback();
        }
        now = end;
    }
    let subscription;
    const calls = [];
    let value = {
        returnValue: true, enabled: false, power: 'Active', senderRunning: false, receiverConnected: false, stopping: false,
        error: null, lastLog: null,
        capabilities: { hardwareH264: options.hardwareH264 === true },
        config: { receiver: '192.168.18.18:19400', source: 'both', compression: 'lz4', autostart: false }
    };
    w.PalmServiceBridge = function () {
        this.cancel = () => { this.cancelled = true; if (options.cancelReentry) this.onservicecallback('{"returnValue":false}'); };
        this.call = (uri, data) => {
            const payload = JSON.parse(data);
            const method = uri.split('/').pop();
            calls.push({ method, payload });
            if (method === 'status') { subscription = this; if (options.silentStatus) return; }
            if (method === 'status' && options.offline) {
                this.onservicecallback(JSON.stringify({ returnValue: false, errorText: 'Service unavailable' }));
                return;
            }
            if (method === 'setConfig') value.config = payload;
            if (method === 'start') { value.enabled = true; value.senderRunning = true; }
            if (method === 'stop' || method === 'cleanup') { value.enabled = false; value.senderRunning = false; }
            if (method === 'cleanup') value.config.autostart = false;
            this.onservicecallback(JSON.stringify({ ...value, subscribed: method === 'status' }));
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
        w, calls, options, subscription: () => subscription,
        advance: milliseconds => settle(() => advance(milliseconds)),
        click: id => settle(() => w.document.getElementById(id).click()),
        event: name => settle(() => w.dispatchEvent(new w.Event(name))),
        reply: (bridge, text) => settle(() => bridge.onservicecallback(text)),
        notify: () => settle(() => subscription.onservicecallback(JSON.stringify({ ...value, subscribed: true }))),
        get value() { return value; },
        el: id => w.document.getElementById(id),
        focus: () => w.document.activeElement.id,
        key: code => settle(() => w.document.activeElement.dispatchEvent(new w.KeyboardEvent('keydown', { keyCode: code, bubbles: true, cancelable: true }))),
        mutations: () => calls.filter(call => call.method !== 'status')
    };
}

test('remote arrows move through setting rows without changing any setting', async t => {
    const f = await fixture(t);
    assert.equal(f.focus(), 'primary');
    await f.key(39);
    assert.equal(f.focus(), 'receiver');
    for (const row of ['source', 'compression', 'autostart', 'metrics']) {
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

test('OK opens choices, arrows only focus, Back cancels, and Save alone persists', async t => {
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
    assert.equal(f.el('modal').hidden, true);
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

test('receiver editing is explicit and cancels without overwriting the address', async t => {
    const f = await fixture(t);
    f.el('receiver').focus();
    await f.click('receiver');
    assert.equal(f.focus(), 'receiver-input');
    f.el('receiver-input').value = '192.168.1.50:19400';
    await f.key(461);
    assert.equal(f.el('receiver-value').textContent, '192.168.18.18:19400');
    await f.click('receiver');
    f.el('receiver-input').value = '192.168.1.50:19400';
    await f.key(39);
    assert.equal(f.focus(), 'receiver-input', 'left/right remain cursor keys inside the editor');
    await f.key(40);
    assert.equal(f.focus(), 'receiver-apply');
    await f.click('receiver-apply');
    assert.equal(f.el('receiver-value').textContent, '192.168.1.50:19400');
    assert.deepEqual(f.mutations(), []);
});

test('save-and-enable applies the draft before enabling, with no optimistic streaming claim', async t => {
    const f = await fixture(t);
    await f.click('compression');
    await f.click('choice-h264-software');
    assert.equal(f.el('primary').textContent, 'Save and enable');
    await f.click('primary');
    assert.deepEqual(f.mutations().map(call => call.method), ['setConfig', 'start']);
    assert.equal(f.value.config.compression, 'h264-software');
    assert.equal(f.el('status-title').textContent, 'Waiting for Prysm');
    assert.equal(f.el('primary').textContent, 'Disable sender');
    await f.click('primary');
    assert.equal(f.value.enabled, false);
});

test('cleanup is tucked away and requires explicit confirmation; Back returns one level', async t => {
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
    assert.equal(f.el('modal').hidden, true);
});

test('status subscription preserves an unsaved choice and its focus', async t => {
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

test('Back on the main screen asks before closing; modal Tab stays inside', async t => {
    const f = await fixture(t);
    await f.key(461);
    assert.equal(f.el('dialog-title').textContent, 'Close Prysm Capture?');
    assert.equal(f.focus(), 'stay');
    f.el('dialog-back').focus();
    await f.key(9);
    assert.equal(f.focus(), 'stay');
    await f.key(27);
    assert.equal(f.el('modal').hidden, true);
    assert.equal(f.focus(), 'primary');
});

test('unavailable service offers setup and keeps the error visible', async t => {
    const f = await fixture(t, { offline: true });
    assert.equal(f.el('primary').textContent, 'Set up service');
    assert.equal(f.el('notice').getAttribute('role'), 'alert');
    assert.equal(f.el('save').disabled, true);
});

test('a waiting sender is described as enabled, rather than actively capturing or paused', async t => {
    const f = await fixture(t);
    assert.equal(f.el('primary').textContent, 'Enable sender');
    await f.click('primary');
    f.value.lastLog = 'capture handshake failed: Connection refused';
    await f.notify();
    assert.equal(f.el('primary').textContent, 'Disable sender');
    assert.equal(f.el('connection').textContent, 'Waiting for Prysm');
    assert.equal(f.el('status-title').textContent, 'Waiting for Prysm');
    assert.match(f.el('status-description').textContent, /192\.168\.18\.18:19400/);
    assert.equal(f.value.enabled, true, 'egui stop must leave the sender available for the next start');
    f.value.receiverConnected = true;
    await f.notify();
    assert.equal(f.el('connection').textContent, 'Connected · Capture off');
    assert.equal(f.el('status-title').textContent, 'Connected · Capture off');
    f.value.capturing = true;
    await f.notify();
    assert.equal(f.el('status-title').textContent, 'Capturing');
    f.value.receiverConnected = false;
    await f.notify();
    assert.equal(f.el('connection').textContent, 'Waiting for Prysm');
});

test('one status subscription recovers after service failure without losing draft settings', async t => {
    const f = await fixture(t);
    assert.deepEqual(f.calls.filter(call => call.method === 'status').map(call => call.payload), [{ subscribe: true }]);
    await f.click('compression'); await f.click('choice-h264-software');
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

test('a silent activation times out; hiding the page cancels retries and synchronous cancel callbacks', async t => {
    const f = await fixture(t, { silentStatus: true, cancelReentry: true });
    await f.advance(30000);
    assert.equal(f.el('connection').textContent, 'TV service unavailable');
    await f.event('pagehide');
    const calls = f.calls.length;
    await f.advance(60000);
    assert.equal(f.calls.length, calls);
    f.options.silentStatus = false;
    await f.event('pageshow');
    assert.equal(f.el('connection').textContent, 'Sender disabled');
});

test('malformed or unsubscribed status replies are cancelled and retried', async t => {
    const f = await fixture(t);
    await f.reply(f.subscription(), '{');
    assert.equal(f.el('connection').textContent, 'TV service unavailable');
    await f.advance(3000);
    await f.reply(f.subscription(), JSON.stringify({ ...f.value, subscribed: false }));
    assert.equal(f.el('connection').textContent, 'TV service unavailable');
    await f.advance(3000);
    assert.equal(f.el('connection').textContent, 'Sender disabled');
});


test('metrics checkbox defaults off and only persists on Save', async t => {
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

for (const [compression, label] of [['h264-software', 'H.264 (software)'], ['h264-hardware', 'H.264 (hardware)']]) {
    test(`${label} is saved and restored as a distinct compression choice`, async t => {
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
        assert.equal(f.el('choice-' + compression).getAttribute('aria-pressed'), 'true');
        assert.equal(f.focus(), 'choice-' + compression);
    });
}

test('hardware compression is offered only when the service advertises support', async t => {
    const f = await fixture(t);
    for (const capabilities of [undefined, { hardwareH264: false }, { hardwareH264: 'true' }]) {
        f.value.capabilities = capabilities;
        await f.notify();
        await f.click('compression');
        assert.equal(f.el('choice-h264-hardware'), null);
        assert.ok(f.el('choice-h264-software'));
        await f.key(461);
    }
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
    assert.doesNotMatch(script, /Local development: simulated TV service|Simulated TV|import\.meta/);
    for (const name of ['app.css', 'appinfo.json', 'icon.png']) {
        assert.ok(fs.existsSync(path.join(__dirname, '../dist', name)));
    }
    assert.equal(JSON.parse(fs.readFileSync(path.join(__dirname, '../dist/appinfo.json'))).main, 'index.html');
});

test('local development service supports settings, subscriptions, start, stop and cleanup', async t => {
    const f = await fixture(t, { devService: true });
    assert.equal(f.el('status-title').textContent, 'Sender disabled');
    await f.click('autostart');
    await f.click('choice-true');
    await f.click('primary');
    assert.equal(f.el('saved-state').textContent, 'Saved');
    assert.equal(f.el('status-title').textContent, 'Capturing');
    await f.click('primary');
    assert.equal(f.el('status-title').textContent, 'Sender disabled');
    await f.click('service-options');
    await f.click('diagnostics');
    assert.match(f.el('dialog-body').textContent, /Local development: simulated TV service/);
    await f.key(27);
    await f.click('cleanup');
    await f.click('confirm-cleanup');
    assert.equal(f.el('autostart-value').textContent, 'Off');
    await f.event('pagehide');
    await f.event('pageshow');
    assert.equal(f.el('status-title').textContent, 'Sender disabled');
});
