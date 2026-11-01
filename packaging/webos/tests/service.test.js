'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const child = require('node:child_process');
const EventEmitter = require('node:events');
const config = require('../service/config');
const Supervisor = require('../service/supervisor');
const permissions = require('../service/permissions');
const paths = require('../service/paths');
const activate = require('../service/activate');
const valid = { receiver: '192.168.1.10:19400', source: 'both', compression: 'lz4', autostart: true };

test('activation rejects failed or empty Luna replies instead of reporting success', t => {
    var response = JSON.stringify({ returnValue: true });
    t.mock.method(child, 'execFile', (file, args, options, callback) => {
        assert.equal(file, 'luna-send');
        assert.equal(args[4], 'luna://' + paths.id + '/status');
        assert.equal(options.timeout, 10000);
        callback(null, response);
    });
    activate((error, value) => { assert.equal(error, null); assert.equal(value.returnValue, true); });
    response = '';
    activate(error => { assert.ok(error); });
    response = JSON.stringify({ returnValue: false, errorText: 'permission denied' });
    activate(error => { assert.match(error.message, /permission denied/); });
});

test('configuration rejects invalid addresses, unexpected fields and shell input', () => {
    assert.deepEqual(config.args(valid), [valid.receiver, '--source', 'both', '--compression', 'lz4']);
    config.validate({ ...valid, receiver: '[::1]:19400' });
    for (const receiver of ['example.com:19400', '1.2.3.4:0', '1.2.3.4:65536', '::1:19400', '1.2.3.4:1;reboot', '']) {
        assert.throws(() => config.validate({ ...valid, receiver }));
    }
    assert.throws(() => config.validate({ ...valid, args: ['--force-capture'] }));
    assert.throws(() => config.validate({ ...valid, source: ';reboot' }));
    assert.throws(() => config.validate({ ...valid, autostart: 'true' }));
});

function fixture(t) {
    t.mock.timers.enable({ apis: ['setTimeout'] });
    const processes = [];
    const supervisor = new Supervisor({ config: { ...valid }, spawn: args => {
        const process = new EventEmitter();
        process.stderr = new EventEmitter();
        process.stdout = new EventEmitter();
        process.stdio = [null, process.stdout, process.stderr, new EventEmitter()];
        process.signals = [];
        process.kill = signal => process.signals.push(signal);
        process.args = args;
        processes.push(process);
        return process;
    } });
    return { supervisor, processes };
}

test('starts only when active; standby and rapid wake never overlap capturers', t => {
    const { supervisor: s, processes: p } = fixture(t);
    s.reconcile();
    assert.equal(p.length, 0);
    s.setPower('Active');
    assert.equal(p.length, 1);
    s.setPower('Suspend');
    assert.deepEqual(p[0].signals, ['SIGTERM']);
    s.setPower('Active');
    assert.equal(p.length, 1);
    p[0].emit('close', 0, null);
    assert.equal(p.length, 2);
    s.enabled = false;
    s.stop();
    t.mock.timers.tick(5000);
    assert.deepEqual(p[1].signals, ['SIGTERM', 'SIGKILL']);
    p[1].emit('close', null, 'SIGKILL');
    t.mock.timers.tick(10000);
    assert.equal(p.length, 2);
});

test('configuration restart waits for cleanup; a stopped sender stays stopped after wake', t => {
    const { supervisor: s, processes: p } = fixture(t);
    s.setPower('Active');
    s.configure({ ...valid, receiver: '[::1]:19400' });
    assert.equal(p.length, 1);
    p[0].emit('close', 0, null);
    assert.equal(p[1].args[0], '[::1]:19400');
    s.enabled = false;
    s.stop();
    p[1].emit('close', 0, null);
    s.setPower('Suspend');
    s.setPower('Active');
    assert.equal(p.length, 2);
    s.start();
    assert.equal(p.length, 3);
});

test('unexpected exit retries with a delay, and stop cancels the retry', t => {
    const { supervisor: s, processes: p } = fixture(t);
    s.setPower('Active');
    p[0].emit('error', new Error('spawn failed'));
    p[0].emit('close', 1, null);
    assert.equal(s.error, 'spawn failed');
    t.mock.timers.tick(4999);
    assert.equal(p.length, 1);
    t.mock.timers.tick(1);
    assert.equal(p.length, 2);
    p[1].emit('close', 1, null);
    s.enabled = false;
    s.stop();
    t.mock.timers.tick(10000);
    assert.equal(p.length, 2);
});

test('normal session exit starts a fresh process without reporting a crash', t => {
    const { supervisor: s, processes: p } = fixture(t);
    s.setPower('Active');
    p[0].emit('close', 0, null);
    assert.equal(s.error, null);
    t.mock.timers.tick(999);
    assert.equal(p.length, 1);
    t.mock.timers.tick(1);
    assert.equal(p.length, 2);
});

test('receiver availability follows explicit handshake events, not process existence or logs', t => {
    const { supervisor: s, processes: p } = fixture(t);
    s.setPower('Active');
    assert.equal(s.status().receiverConnected, false);
    p[0].stderr.emit('data', Buffer.from('connected to receiver'));
    assert.equal(s.status().receiverConnected, false);
    p[0].stdout.emit('data', Buffer.from('PRYSM_CONNECTED=1\n'));
    assert.equal(s.status().receiverConnected, false);
    p[0].stdio[3].emit('data', Buffer.from('{"type":"con'));
    p[0].stdio[3].emit('data', Buffer.from('nected"}\n'));
    assert.equal(s.status().receiverConnected, true);
    assert.equal(s.status().capturing, false);
    p[0].stdio[3].emit('data', Buffer.from('{"type":"capturing"}\n'));
    assert.equal(s.status().capturing, true);
    p[0].stdio[3].emit('data', Buffer.from('{"type":"disconnected"}\n'));
    assert.equal(s.status().receiverConnected, false);
    assert.equal(s.status().capturing, false);
    assert.deepEqual(p[0].signals, ['SIGTERM']);
    t.mock.timers.tick(5000);
    assert.deepEqual(p[0].signals, ['SIGTERM', 'SIGKILL']);
    p[0].emit('close', null, 'SIGKILL');
    assert.equal(s.error, null, 'normal cleanup timeout is not a receiver failure');
    t.mock.timers.tick(1000);
    p[0].stdio[3].emit('data', Buffer.from('{"type":"connected"}\n'));
    assert.equal(s.status().receiverConnected, false, 'old-process output cannot mark a new session connected');
});

test('Luna setup and cleanup touch only the Prysm manifest and boot hook', t => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'prysm-service-'));
    const original = { ...paths };
    Object.assign(paths, { manifest: path.join(directory, 'prysm.json'), hook: path.join(directory, 'init.d', 'prysm-capture') });
    const calls = [];
    t.mock.method(child, 'execFileSync', (...args) => calls.push(args));
    t.after(() => { Object.assign(paths, original); fs.rmSync(directory, { recursive: true }); });
    permissions.install();
    permissions.install();
    const manifest = JSON.parse(fs.readFileSync(paths.manifest));
    assert.deepEqual(manifest.roleFiles, [paths.service + '/capture.role.json']);
    assert.equal(manifest.serviceFiles, undefined);
    const role = require('../service/capture.role.json');
    assert.equal(role.exeName, paths.binary);
    permissions.autostart(true);
    assert.match(fs.readFileSync(paths.hook, 'utf8'), /if \[ -x .*setup.sh \]/);
    permissions.autostart(false);
    permissions.remove();
    permissions.remove();
    assert.equal(fs.existsSync(paths.hook), false);
    assert.equal(fs.existsSync(paths.manifest), false);
    assert.equal(calls.length, 4);
    assert.ok(calls.every(args => args[0] === 'ls-control' && args[1][0] === 'scan-services'));
    fs.writeFileSync(paths.manifest, '{"id":"unrelated"}');
    assert.throws(() => permissions.install(), /unrelated/);
    assert.throws(() => permissions.remove(), /unrelated/);
    assert.equal(JSON.parse(fs.readFileSync(paths.manifest)).id, 'unrelated');
    fs.writeFileSync(paths.hook, '#!/bin/sh\necho unrelated\n');
    assert.throws(() => permissions.autostart(true), /unrelated/);
    assert.throws(() => permissions.autostart(false), /unrelated/);
});

test('service restricts mutations to its app and serializes cleanup against start', () => {
    const vm = require('node:vm');
    const methods = {};
    const handlers = {};
    const files = { [paths.config]: JSON.stringify(valid) };
    const processes = [];
    let removed = false;
    let installed = 0;
    let failHook = false;
    function Service() {
        this.activityManager = {};
        this.register = (name, handler) => { methods[name] = handler; };
        this.subscribe = () => ({ on: (event, handler) => { handlers[event] = handler; }, cancel() {} });
    }
    const modules = {
        'webos-service': Service,
        fs: {
            existsSync: file => file === paths.state || file in files,
            readFileSync: file => files[file],
            writeFileSync: (file, value) => { files[file] = value; },
            renameSync: (from, to) => { files[to] = files[from]; delete files[from]; }
        },
        child_process: { spawn: () => {
            const process = new EventEmitter();
            process.stderr = new EventEmitter();
            process.stdout = new EventEmitter();
        process.stdio = [null, process.stdout, process.stderr, new EventEmitter()];
            process.kill = () => {};
            processes.push(process);
            return process;
        } },
        './paths': paths, './config': config, './supervisor': Supervisor,
        './permissions': {
            install() { installed++; },
            remove() { removed = true; },
            autostart() { if (failHook) throw new Error('hook write failed'); }
        }
    };
    vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../service/service.js'), 'utf8'), {
        require: name => modules[name], process: { getuid: () => 0, on() {} }, setTimeout
    });
    function request(name, sender, payload = {}) {
        const replies = [];
        methods[name]({ sender, payload, respond: value => replies.push(value) });
        return replies;
    }
    assert.equal(request('start', 'another.app')[0].returnValue, false);
    assert.equal(installed, 0);
    handlers.response({ payload: { returnValue: true, state: 'Active' } });
    assert.equal(processes.length, 1);
    failHook = true;
    assert.equal(request('setConfig', paths.app, { ...valid, receiver: '[::1]:19400' })[0].returnValue, false);
    assert.deepEqual(JSON.parse(files[paths.config]), valid, 'failed hook update restores saved settings');
    failHook = false;
    const cleanup = request('cleanup', paths.app);
    assert.equal(cleanup.length, 0, 'cleanup waits for the native process to close');
    assert.equal(request('start', paths.app)[0].returnValue, false);
    assert.equal(removed, false);
    processes[0].emit('close', 0, null);
    assert.equal(removed, true);
    assert.equal(cleanup[0].enabled, false);
    assert.equal(processes.length, 1, 'cleanup cannot accidentally restart the capturer');
    assert.equal(JSON.parse(files[paths.config]).autostart, false);
});
