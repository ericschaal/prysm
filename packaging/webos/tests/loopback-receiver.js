'use strict';
// Isolated hardware test receiver. Pixels stay on TV loopback and are discarded.
var net = require('net');
var fs = require('fs');
var path = require('path');
var directory = path.dirname(__filename);
var version = Number(fs.readFileSync(path.join(directory, 'wire-version.txt'), 'utf8').trim());
var magic = 'PRY' + version;
var timer = setTimeout(function () { process.exit(1); }, 15000);
var server = net.createServer(function (socket) {
    server.close();
    var pending = Buffer.alloc(0);
    var hello = false;
    var frames = 0;
    socket.on('data', function (data) {
        pending = Buffer.concat([pending, data]);
        if (!hello && pending.length >= 4) {
            var length = pending.readUInt32BE(0);
            if (length < 1 || length > 128) throw new Error('Invalid hello length');
            if (pending.length < 4 + length) return;
            var value = JSON.parse(pending.subarray(4, 4 + length));
            if (value.type !== 'hello' || value.version !== version) throw new Error('Invalid hello');
            pending = pending.subarray(4 + length);
            hello = true;
            var settings = Buffer.from(JSON.stringify({ type: 'capture_settings', refresh_rate_hz: 15, capture_resolution: [320, 180] }));
            var prefix = Buffer.alloc(4);
            prefix.writeUInt32BE(settings.length);
            socket.write(Buffer.concat([prefix, settings]));
        }
        while (hello && pending.length >= 24) {
            if (pending.subarray(0, 4).toString() !== magic || pending.readUInt32BE(4) !== 320 ||
                pending.readUInt32BE(8) !== 180 || pending.readUInt32BE(12) !== 0 ||
                pending.readUInt32BE(16) !== 0 || pending.readUInt32BE(20) !== 320 * 180 * 3) {
                throw new Error('Invalid raw frame header');
            }
            if (pending.length < 24 + 320 * 180 * 3) return;
            pending = pending.subarray(24 + 320 * 180 * 3);
            frames++;
            if (frames === 3) fs.writeFileSync(path.join(directory, 'receiver.complete'), '3\n', { flag: 'wx' });
        }
    });
    socket.on('error', function (error) { console.error(error.message); process.exitCode = 1; });
    socket.on('close', function () { clearTimeout(timer); if (frames < 3) process.exitCode = 1; });
});
server.listen(0, '127.0.0.1', function () {
    fs.writeFileSync(path.join(directory, 'receiver.port'), String(server.address().port), { flag: 'wx' });
});
