#!/bin/sh
# Smoke-tests a prysm-headless image: starts it without a settings file
# (healthy, and the file not written), then with a null output, waits for the
# HEALTHCHECK, reads the portal state, checks a clean SIGTERM exit each time.
# Usage: smoke.sh IMAGE
set -eu
image=${1:?usage: smoke.sh IMAGE}
name=prysm-headless-smoke-$$
volume=$name-data

cleanup() {
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker volume rm "$volume" >/dev/null 2>&1 || true
}
trap cleanup EXIT

fail() {
    echo "smoke: $*" >&2
    docker logs "$name" >&2 || true
    exit 1
}

# Starts the container, waits for the HEALTHCHECK and sets $port.
start() {
    docker run -d --name "$name" --read-only --cap-drop ALL \
        --security-opt no-new-privileges:true -v "$volume:/data" -p 127.0.0.1::8787 \
        "$image" >/dev/null
    health=starting
    for _ in $(seq 60); do
        health=$(docker inspect -f '{{.State.Health.Status}}' "$name")
        [ "$health" = starting ] || break
        sleep 1
    done
    [ "$health" = healthy ] || fail "container is $health, expected healthy"
    port=$(docker port "$name" 8787/tcp | head -n 1 | sed 's/.*://')
}

# Stops it with SIGTERM, expects a clean exit, and removes it.
stop() {
    docker stop -t 30 "$name" >/dev/null
    code=$(docker inspect -f '{{.State.ExitCode}}' "$name")
    [ "$code" = 0 ] || fail "exit code $code after SIGTERM"
    docker rm "$name" >/dev/null
}

# A shell command in the volume, as the image's user.
in_volume() {
    docker run --rm --entrypoint sh -v "$volume:/data" "$image" -c "$1"
}

docker volume create "$volume" >/dev/null

# No settings file: unconfigured, idle and healthy; the file is written only
# by a save in the portal.
start
curl -fsS "http://127.0.0.1:$port/api/v1/state" >/dev/null || fail "GET /api/v1/state failed without settings"
stop
in_volume 'test ! -e /data/settings.json' || fail "settings.json was written without a save"

# Settings seeded the way an operator's file would be.
in_volume 'printf "{\n  \"output\": { \"kind\": \"null\" }\n}\n" > /data/settings.json'
start
state=$(curl -fsS "http://127.0.0.1:$port/api/v1/state") || fail "GET /api/v1/state failed"
echo "$state" | grep -q '"null"' || fail "state does not show the null output: $state"
curl -fsS "http://127.0.0.1:$port/" | grep -qi '<html' || fail "portal page missing"
stop
echo "smoke: ok ($image)"
