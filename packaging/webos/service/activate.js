'use strict';

var child = require('child_process');
var paths = require('./paths');

function activate(callback) {
    // Keep stdin open: this TV's luna-send exits before replying on EOF.
    return child.execFile('luna-send', ['-w', '5000', '-n', '1', 'luna://' + paths.id + '/status', '{}'],
        { timeout: 10000 }, function (error, stdout) {
            if (error) return callback(error);
            try {
                var value = JSON.parse(stdout);
                if (!value.returnValue) throw new Error(value.errorText || 'Service activation failed');
            } catch (failure) { return callback(failure); }
            callback(null, value);
        });
}

module.exports = activate;
if (require.main === module) {
    activate(function (error, value) {
        if (error) { console.error(error.message); process.exitCode = 1; }
        else console.log(JSON.stringify(value));
    });
}
