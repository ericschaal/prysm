'use strict';

var Service = require('webos-service');
var fs = require('fs');
var child = require('child_process');
var paths = require('./paths');
var config = require('./config');
var permissions = require('./permissions');
var Supervisor = require('./supervisor');

if (process.getuid() !== 0) throw new Error('Open Prysm Capture and run service setup first');
var saved = fs.existsSync(paths.config) ? config.validate(JSON.parse(fs.readFileSync(paths.config, 'utf8'))) : config.defaults;
var supervisor = new Supervisor({ config: saved, spawn: function (args) {
    return child.spawn(paths.binary, args, { stdio: ['ignore', 'ignore', 'pipe', 'pipe'], env: Object.assign({}, process.env, { PRYSM_EVENTS_FD: '3' }) });
} });
var service = new Service(paths.id, undefined, { noBuiltinMethods: true });
// Keep the supervisor available without requesting an activity that could prevent standby.
service.activityManager.exitOnTimeout = false;
var changing = false;

function persist(value) {
    if (!fs.existsSync(paths.state)) fs.mkdirSync(paths.state, { mode: 0o700 });
    var temporary = paths.config + '.tmp';
    fs.writeFileSync(temporary, JSON.stringify(value) + '\n', { mode: 0o600 });
    fs.renameSync(temporary, paths.config);
}

function register(name, action) {
    service.register(name, function (message) {
        try {
            if (name !== 'status' && message.sender !== paths.app) throw new Error('Only the Prysm Capture app may change service state');
            if (name !== 'status') {
                if (changing) throw new Error('A service change is already in progress');
                changing = true;
                var respond = message.respond.bind(message);
                message.respond = function (value) { changing = false; return respond(value); };
            }
            action(message);
        } catch (error) {
            message.respond({ returnValue: false, errorText: error.message });
        }
    });
}

register('status', function (message) { message.respond(supervisor.status()); });
register('start', function (message) {
    permissions.install();
    supervisor.start();
    message.respond(supervisor.status());
});
register('stop', function (message) {
    supervisor.enabled = false;
    supervisor.stop(function () { message.respond(supervisor.status()); });
});
register('setConfig', function (message) {
    var value = config.validate(message.payload);
    persist(value);
    try {
        permissions.autostart(value.autostart);
    } catch (error) {
        if (supervisor.config.receiver) persist(supervisor.config);
        else fs.unlinkSync(paths.config);
        throw error;
    }
    supervisor.configure(value);
    message.respond(supervisor.status());
});
register('cleanup', function (message) {
    supervisor.enabled = false;
    var value = Object.assign({}, supervisor.config, { autostart: false });
    permissions.autostart(false);
    // Unconfigured first-run defaults have no valid receiver to persist.
    if (value.receiver) persist(value);
    supervisor.config = value;
    supervisor.stop(function () {
        try {
            permissions.remove();
            message.respond(supervisor.status());
        } catch (error) {
            message.respond({ returnValue: false, errorText: error.message });
        }
    });
});

function watchPower() {
    var subscription = service.subscribe('luna://com.webos.service.tvpower/power/getPowerState', { subscribe: true });
    subscription.on('response', function (message) {
        var value = message.payload;
        if (value.returnValue === false || typeof value.state !== 'string') {
            supervisor.error = 'TV power subscription failed: ' + JSON.stringify(value);
            supervisor.setPower(null);
            subscription.cancel();
            setTimeout(watchPower, 5000);
        } else {
            supervisor.setPower(value.state);
        }
    });
}
watchPower();
['SIGINT', 'SIGTERM'].forEach(function (signal) {
    process.on(signal, function () {
        supervisor.enabled = false;
        supervisor.stop(function () { process.exit(0); });
    });
});
