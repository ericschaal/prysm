#!/bin/sh
set -eu
service=/media/developer/apps/usr/palm/services/io.prysm.capture.service
elevate=/media/developer/apps/usr/palm/services/org.webosbrew.hbchannel.service/elevate-service
test "$(id -u)" = 0
test -x "$elevate"
"$elevate" io.prysm.capture.service io.prysm.capture
node "$service/permissions.js"
node "$service/activate.js"
