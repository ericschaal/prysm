#!/bin/sh
set -eu
service=/media/developer/apps/usr/palm/services/io.prysm.capture.service
elevate=/media/developer/apps/usr/palm/services/org.webosbrew.hbchannel.service/elevate-service
test "$(id -u)" = 0
if [ ! -f "$service/supervisor.role.json" ]; then
  test -x "$elevate"
  "$elevate" io.prysm.capture.service io.prysm.capture
fi
node "$service/permissions.js"
node "$service/activate.js"
