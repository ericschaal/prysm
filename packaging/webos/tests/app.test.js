'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('../../../apps/prysm-web/control-ui/node_modules/jsdom');
const html = fs.readFileSync(path.join(__dirname, '../app/index.html'), 'utf8');
const script = fs.readFileSync(path.join(__dirname, '../app/app.js'), 'utf8');

function fixture(t, options = {}) {
    const dom = new JSDOM(html, { runScripts: 'outside-only', url: 'http://localhost/' });
    t.after(() => dom.window.close());
    const w = dom.window;
    let poll;
    w.setInterval = callback => { poll = callback; return 1; };
    const calls = [];
    let value = {
        returnValue: true, enabled: false, power: 'Active', senderRunning: false, receiverConnected: false, stopping: false,
        error: null, lastLog: null,
        config: { receiver: '192.168.18.18:19400', source: 'both', compression: 'lz4', autostart: false }
    };
    w.PalmServiceBridge = function () {
        this.cancel = () => {};
        this.call = (uri, data) => {
            const payload = JSON.parse(data);
            const method = uri.split('/').pop();
            calls.push({ method, payload });
            if (method === 'status' && options.offline) {
                this.onservicecallback(JSON.stringify({ returnValue: false, errorText: 'Service unavailable' }));
                return;
            }
            if (method === 'setConfig') value.config = payload;
            if (method === 'start') { value.enabled = true; value.senderRunning = true; }
            if (method === 'stop' || method === 'cleanup') { value.enabled = false; value.senderRunning = false; }
            if (method === 'cleanup') value.config.autostart = false;
            this.onservicecallback(JSON.stringify(value));
        };
    };
    w.eval(script);
    return {
        w, calls, poll: () => poll(), get value() { return value; },
        el: id => w.document.getElementById(id),
        focus: () => w.document.activeElement.id,
        key: code => w.document.activeElement.dispatchEvent(new w.KeyboardEvent('keydown', { keyCode: code, bubbles: true, cancelable: true })),
        mutations: () => calls.filter(call => call.method !== 'status')
    };
}

test('remote arrows move through setting rows without changing any setting', t => {
    const f = fixture(t);
    assert.equal(f.focus(), 'primary');
    f.key(39);
    assert.equal(f.focus(), 'receiver');
    for (const row of ['source', 'compression', 'autostart']) {
        f.key(40);
        assert.equal(f.focus(), row);
    }
    f.key(39);
    f.key(40); // Unchanged settings have no enabled Save action.
    assert.equal(f.focus(), 'autostart');
    assert.equal(f.w.document.querySelectorAll('select').length, 0);
    assert.equal(f.el('source-value').textContent, 'Video + TV menus');
    assert.equal(f.el('compression-value').textContent, 'LZ4');
    assert.equal(f.el('autostart-value').textContent, 'Off');
    assert.deepEqual(f.mutations(), []);
});

test('OK opens choices, arrows only focus, Back cancels, and Save alone persists', t => {
    const f = fixture(t);
    f.el('source').focus();
    f.el('source').click();
    assert.equal(f.focus(), 'choice-both');
    f.key(40);
    assert.equal(f.focus(), 'choice-video');
    assert.equal(f.el('choice-both').getAttribute('aria-pressed'), 'true');
    assert.equal(f.value.config.source, 'both');
    f.key(461);
    assert.equal(f.el('modal').hidden, true);
    assert.equal(f.focus(), 'source');
    assert.equal(f.el('source-value').textContent, 'Video + TV menus');
    f.el('source').click();
    f.el('choice-video').click();
    assert.equal(f.el('source-value').textContent, 'Video only');
    assert.equal(f.el('saved-state').textContent, 'Unsaved changes');
    assert.equal(f.value.config.source, 'both');
    assert.deepEqual(f.mutations(), []);
    f.el('save').click();
    assert.equal(f.value.config.source, 'video');
    assert.equal(f.mutations().length, 1);
    assert.equal(f.mutations()[0].method, 'setConfig');
    assert.equal(f.el('save').disabled, true);
});

test('receiver editing is explicit and cancels without overwriting the address', t => {
    const f = fixture(t);
    f.el('receiver').focus();
    f.el('receiver').click();
    assert.equal(f.focus(), 'receiver-input');
    f.el('receiver-input').value = '192.168.1.50:19400';
    f.key(461);
    assert.equal(f.el('receiver-value').textContent, '192.168.18.18:19400');
    f.el('receiver').click();
    f.el('receiver-input').value = '192.168.1.50:19400';
    f.key(39);
    assert.equal(f.focus(), 'receiver-input', 'left/right remain cursor keys inside the editor');
    f.key(40);
    assert.equal(f.focus(), 'receiver-apply');
    f.el('receiver-apply').click();
    assert.equal(f.el('receiver-value').textContent, '192.168.1.50:19400');
    assert.deepEqual(f.mutations(), []);
});

test('save-and-enable applies the draft before enabling, with no optimistic streaming claim', t => {
    const f = fixture(t);
    f.el('compression').click();
    f.el('choice-h264').click();
    assert.equal(f.el('primary').textContent, 'Save and enable');
    f.el('primary').click();
    assert.deepEqual(f.mutations().map(call => call.method), ['setConfig', 'start']);
    assert.equal(f.value.config.compression, 'h264');
    assert.equal(f.el('status-title').textContent, 'Waiting for Prysm');
    assert.equal(f.el('primary').textContent, 'Disable sender');
    f.el('primary').click();
    assert.equal(f.value.enabled, false);
});

test('cleanup is tucked away and requires explicit confirmation; Back returns one level', t => {
    const f = fixture(t);
    f.el('service-options').focus();
    f.el('service-options').click();
    f.el('diagnostics').click();
    f.key(461);
    assert.equal(f.el('dialog-title').textContent, 'Service options');
    f.el('cleanup').click();
    assert.equal(f.focus(), 'keep-setup');
    f.key(40);
    assert.equal(f.focus(), 'confirm-cleanup');
    assert.deepEqual(f.mutations(), []);
    f.key(461);
    assert.equal(f.el('dialog-title').textContent, 'Service options');
    f.el('cleanup').click();
    f.el('confirm-cleanup').click();
    assert.equal(f.mutations().length, 1);
    assert.equal(f.mutations()[0].method, 'cleanup');
    assert.equal(f.el('modal').hidden, true);
});

test('status polling preserves an unsaved choice and its focus', t => {
    const f = fixture(t);
    f.el('autostart').focus();
    f.el('autostart').click();
    f.el('choice-true').click();
    assert.equal(f.focus(), 'autostart');
    f.poll();
    assert.equal(f.focus(), 'autostart');
    assert.equal(f.el('autostart-value').textContent, 'On');
    assert.equal(f.value.config.autostart, false);
    assert.equal(f.el('saved-state').textContent, 'Unsaved changes');
});

test('Back on the main screen asks before closing; modal Tab stays inside', t => {
    const f = fixture(t);
    f.key(461);
    assert.equal(f.el('dialog-title').textContent, 'Close Prysm Capture?');
    assert.equal(f.focus(), 'stay');
    f.el('dialog-back').focus();
    f.key(9);
    assert.equal(f.focus(), 'stay');
    f.key(27);
    assert.equal(f.el('modal').hidden, true);
    assert.equal(f.focus(), 'primary');
});

test('unavailable service offers setup and keeps the error visible', t => {
    const f = fixture(t, { offline: true });
    assert.equal(f.el('primary').textContent, 'Set up service');
    assert.equal(f.el('notice').getAttribute('role'), 'alert');
    assert.equal(f.el('save').disabled, true);
});

test('a waiting sender is described as enabled, rather than actively capturing or paused', t => {
    const f = fixture(t);
    assert.equal(f.el('primary').textContent, 'Enable sender');
    f.el('primary').click();
    f.value.lastLog = 'capture handshake failed: Connection refused';
    f.poll();
    assert.equal(f.el('primary').textContent, 'Disable sender');
    assert.equal(f.el('connection').textContent, 'Waiting for Prysm');
    assert.equal(f.el('status-title').textContent, 'Waiting for Prysm');
    assert.match(f.el('status-description').textContent, /192\.168\.18\.18:19400/);
    assert.equal(f.value.enabled, true, 'egui stop must leave the sender available for the next start');
    f.value.receiverConnected = true;
    f.poll();
    assert.equal(f.el('connection').textContent, 'Connected · Capture off');
    assert.equal(f.el('status-title').textContent, 'Connected · Capture off');
    f.value.capturing = true;
    f.poll();
    assert.equal(f.el('status-title').textContent, 'Capturing');
    f.value.receiverConnected = false;
    f.poll();
    assert.equal(f.el('connection').textContent, 'Waiting for Prysm');
});
