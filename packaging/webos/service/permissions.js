'use strict';

var fs = require('fs');
var child = require('child_process');
var paths = require('./paths');
var path = require('path');

function manifest() {
    return {
        id: 'io.prysm.capture.native',
        version: '1.0.0',
        roleFiles: [paths.service + '/capture.role.json']
    };
}

function removeFile(file) {
    try { fs.unlinkSync(file); } catch (error) {
        if (error.code !== 'ENOENT') throw error;
    }
}

function scan() {
    child.execFileSync('ls-control', ['scan-services'], { timeout: 10000 });
}

function sameManifest(value, expected) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
    var keys = Object.keys(value).sort();
    return JSON.stringify(keys) === JSON.stringify(Object.keys(expected).sort()) &&
        keys.every(function (key) { return JSON.stringify(value[key]) === JSON.stringify(expected[key]); });
}

function supervisorManifest() {
    return { id: 'io.prysm.capture.supervisor.native', version: '1.0.0',
        roleFiles: [paths.service + '/supervisor.role.json'],
        serviceFiles: [paths.service + '/supervisor.service'],
        clientPermissionFiles: [paths.service + '/supervisor.client.json'],
        apiPermissionFiles: [paths.service + '/supervisor.api.json'] };
}

function supervisorRegistration() {
    var expected = supervisorManifest();
    var native = fs.existsSync(paths.service + '/supervisor.role.json');
    if (fs.existsSync(paths.supervisorManifest)) {
        if (!sameManifest(JSON.parse(fs.readFileSync(paths.supervisorManifest, 'utf8')), expected)) {
            throw new Error('Refusing to change an unrelated supervisor manifest');
        }
        if (!native) removeFile(paths.supervisorManifest);
    } else if (native) {
        fs.writeFileSync(paths.supervisorManifest, JSON.stringify(expected) + '\n', { flag: 'wx', mode: 0o644 });
    }
}

function install() {
    var expected = JSON.stringify(manifest());
    if (fs.existsSync(paths.manifest)) {
        if (!sameManifest(JSON.parse(fs.readFileSync(paths.manifest, 'utf8')), manifest())) {
            throw new Error('Refusing to replace an unrelated Luna manifest: ' + paths.manifest);
        }
    } else {
        fs.writeFileSync(paths.manifest, expected + '\n', { flag: 'wx', mode: 0o644 });
    }
    supervisorRegistration();
    scan();
}

function remove() {
    if (fs.existsSync(paths.manifest)) {
        if (!sameManifest(JSON.parse(fs.readFileSync(paths.manifest, 'utf8')), manifest())) {
            throw new Error('Refusing to remove an unrelated Luna manifest');
        }
        removeFile(paths.manifest);
    }
    scan();
}

function autostart(enabled) {
    var script = '#!/bin/sh\nif [ -x ' + paths.service + '/setup.sh ]; then\n  ' +
        paths.service + '/setup.sh > /tmp/prysm-capture-setup.log 2>&1 &\nfi\n';
    if (fs.existsSync(paths.hook) && fs.readFileSync(paths.hook, 'utf8') !== script) {
        throw new Error('Refusing to change an unrelated boot hook');
    }
    if (!enabled) return removeFile(paths.hook);
    if (!fs.existsSync(path.dirname(paths.hook))) {
        fs.mkdirSync(path.dirname(paths.hook), { mode: 0o755 });
    }
    // The guard makes a leftover hook harmless if the IPK is removed first.
    fs.writeFileSync(paths.hook, script, { mode: 0o755 });
}

module.exports = { install: install, remove: remove, autostart: autostart, manifest: manifest };
if (require.main === module) {
    if (process.getuid() !== 0) throw new Error('Luna setup requires root');
    install();
}
