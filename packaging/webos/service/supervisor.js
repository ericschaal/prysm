'use strict';

var config = require('./config');

function Supervisor(options) {
    this.options = options;
    this.config = options.config;
    this.enabled = options.config.autostart;
    this.power = null;
    this.child = null;
    this.receiverConnected = false;
    this.capturing = false;
    this.stopping = false;
    this.error = null;
    this.lastLog = null;
    this.retry = null;
}

Supervisor.prototype.status = function () {
    return { enabled: this.enabled, power: this.power, senderRunning: !!this.child,
        receiverConnected: this.receiverConnected, capturing: this.capturing, stopping: this.stopping, error: this.error, lastLog: this.lastLog, config: this.config };
};

Supervisor.prototype.reconcile = function () {
    var self = this;
    var wanted = this.enabled && this.power === 'Active' && !!this.config.receiver;
    if (!wanted) return this.stop();
    if (this.child || this.stopping || this.retry) return;
    var process = this.options.spawn(config.args(this.config));
    this.child = process;
    this.receiverConnected = false;
    this.error = null;
    this.lastLog = null;
    this.capturing = false;
    var expectedExit = false;
    var pending = '';
    process.stdio[3].on('data', function (data) {
        if (self.child !== process) return;
        pending += data.toString();
        var lines = pending.split('\n');
        pending = lines.pop();
        lines.forEach(function (line) {
            var event;
            try { event = JSON.parse(line); if (!event || typeof event.type !== 'string') throw new Error('Invalid status'); } catch (_) { self.error = 'Invalid capturer status'; self.stop(); return; }
            if (event.type === 'connected') self.receiverConnected = true;
            else if (event.type === 'capturing') self.capturing = true;
            else if (event.type === 'disconnected' || event.type === 'failed') {
                expectedExit = event.type === 'disconnected';
                self.receiverConnected = false;
                self.capturing = false;
                if (event.type === 'failed') self.error = event.message;
                self.stop();
            } else { self.error = 'Unknown capturer status'; self.stop(); }
        });
        if (pending.length > 4096) { pending = ''; self.error = 'Oversized capturer status'; self.stop(); }
    });
    process.stderr.on('data', function (data) {
        self.lastLog = data.toString().slice(-2000);
    });
    process.on('error', function (error) { self.error = error.message; });
    process.on('close', function (code, signal) {
        if (self.child !== process) return;
        self.child = null;
        self.receiverConnected = false;
        self.capturing = false;
        self.stopping = false;
        clearTimeout(self.killTimer);
        var callbacks = self.stopCallbacks || [];
        self.stopCallbacks = [];
        if (!callbacks.length && self.enabled && self.power === 'Active') {
            if (code !== 0 && !expectedExit) self.error = self.error || 'Capturer exited: ' + code + ' ' + signal;
            self.retry = setTimeout(function () { self.retry = null; self.reconcile(); }, code === 0 || expectedExit ? 1000 : 5000);
        }
        callbacks.forEach(function (callback) { callback(); });
        self.reconcile();
    });
};

Supervisor.prototype.stop = function (callback) {
    clearTimeout(this.retry);
    this.retry = null;
    if (!this.child) { if (callback) callback(); return; }
    if (callback) (this.stopCallbacks || (this.stopCallbacks = [])).push(callback);
    if (this.stopping) return;
    this.stopping = true;
    var process = this.child;
    process.kill('SIGTERM');
    this.killTimer = setTimeout(function () { process.kill('SIGKILL'); }, 5000);
};

Supervisor.prototype.setPower = function (state) {
    this.power = state;
    var self = this;
    if (state !== 'Active') this.stop(function () { self.reconcile(); });
    else this.reconcile();
};

Supervisor.prototype.start = function () {
    config.validate(this.config);
    this.enabled = true;
    this.reconcile();
};

Supervisor.prototype.configure = function (value) {
    this.config = config.validate(value);
    var self = this;
    this.stop(function () { self.reconcile(); });
};

module.exports = Supervisor;
