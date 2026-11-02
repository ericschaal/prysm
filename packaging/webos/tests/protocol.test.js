'use strict';
const assert = require('node:assert/strict');
const { EventEmitter } = require('node:events');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

test('TV smoke receiver follows the shared version file after a protocol bump', function () {
    const current = Number(fs.readFileSync(path.join(__dirname, '../../../crates/capture/prysm-webos-protocol/wire-version.txt'), 'utf8').trim());
    const version = (current + 1) % 10;
    const files = new Map();
    let accept;
    const server = {
        close() {},
        listen(port, host, ready) { ready(); },
        address() { return { port: 12345 }; },
    };
    vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'loopback-receiver.js'), 'utf8'), {
        __filename: '/probe/receiver.js',
        Buffer,
        console,
        setTimeout() { return 1; },
        clearTimeout() {},
        process: { exit() { throw new Error('Unexpected receiver exit'); } },
        require(name) {
            if (name === 'net') return { createServer(callback) { accept = callback; return server; } };
            if (name === 'path') return path;
            assert.equal(name, 'fs');
            return {
                readFileSync(file) { assert.equal(file, '/probe/wire-version.txt'); return version + '\n'; },
                writeFileSync(file, value) { files.set(file, value); },
            };
        },
    });
    const socket = new EventEmitter();
    const replies = [];
    socket.write = data => replies.push(data);
    accept(socket);
    const hello = Buffer.from(JSON.stringify({ type: 'hello', version }));
    const prefix = Buffer.alloc(4);
    prefix.writeUInt32BE(hello.length);
    socket.emit('data', Buffer.concat([prefix, hello]));
    assert.equal(JSON.parse(replies[0].subarray(4)).type, 'capture_settings');
    const frame = Buffer.alloc(24 + 320 * 180 * 3);
    frame.write('PRY' + version);
    frame.writeUInt32BE(320, 4);
    frame.writeUInt32BE(180, 8);
    frame.writeUInt32BE(frame.length - 24, 20);
    for (let count = 0; count < 3; count++) socket.emit('data', frame);
    socket.emit('close');
    assert.equal(files.get('/probe/receiver.complete'), '3\n');
});
