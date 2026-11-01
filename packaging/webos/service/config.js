'use strict';

var net = require('net');
var defaults = { receiver: '', source: 'both', compression: 'lz4', autostart: false };

function validate(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value) ||
        Object.keys(value).some(function (key) { return !Object.prototype.hasOwnProperty.call(defaults, key); })) {
        throw new Error('Invalid capture configuration');
    }
    var match = typeof value.receiver === 'string' && /^(\[[^\]]+\]|[^:]+):([0-9]+)$/.exec(value.receiver);
    var host = match && match[1].replace(/^\[|\]$/g, '');
    if (!match || !net.isIP(host) || Number(match[2]) < 1 || Number(match[2]) > 65535 ||
        (net.isIP(host) === 6 && match[1][0] !== '[')) {
        throw new Error('Receiver must be an IP:port or [IPv6]:port, with port 1–65535');
    }
    if (['video', 'ui', 'both'].indexOf(value.source) < 0 ||
        ['none', 'lz4', 'h264'].indexOf(value.compression) < 0 || typeof value.autostart !== 'boolean') {
        throw new Error('Invalid source, compression, or autostart');
    }
    return { receiver: value.receiver, source: value.source, compression: value.compression, autostart: value.autostart };
}

function args(config) {
    validate(config);
    return [config.receiver, '--source', config.source, '--compression', config.compression];
}

module.exports = { defaults: defaults, validate: validate, args: args };
