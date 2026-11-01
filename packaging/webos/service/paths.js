'use strict';

var service = '/media/developer/apps/usr/palm/services/io.prysm.capture.service';
module.exports = {
    id: 'io.prysm.capture.service',
    app: 'io.prysm.capture',
    service: service,
    binary: service + '/prysm-webos-capture',
    state: '/var/lib/prysm-capture',
    config: '/var/lib/prysm-capture/config.json',
    hook: '/var/lib/webosbrew/init.d/prysm-capture',
    manifest: '/var/luna-service2/manifests.d/io.prysm.capture.native.json',
    supervisorManifest: '/var/luna-service2/manifests.d/io.prysm.capture.supervisor.native.json'
};
