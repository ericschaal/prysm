#!/bin/sh
# Run as root by the TV app through Homebrew Channel `exec`: register the Luna
# roles and boot hook, rescan and activate the service.
set -eu
exec /media/developer/apps/usr/palm/services/io.prysm.tv.service/prysm-webos-service setup
