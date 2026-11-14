#!/bin/sh
# Smoke-tests a prysm-headless image: starts it with a null output and a
# missing-then-written settings file, waits for the HEALTHCHECK, reads the
# portal state, checks a clean SIGTERM exit. Usage: smoke.sh IMAGE
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

docker volume create "$volume" >/dev/null
# Seed the settings as the image's user, the way an operator's file would be.
docker run --rm --entrypoint sh -v "$volume:/data" "$image" -c \
    'printf "{\n  \"output\": { \"kind\": \"null\" }\n}\n" > /data/settings.json'

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
state=$(curl -fsS "http://127.0.0.1:$port/api/v1/state") || fail "GET /api/v1/state failed"
echo "$state" | grep -q '"null"' || fail "state does not show the null output: $state"
curl -fsS "http://127.0.0.1:$port/" | grep -qi '<html' || fail "portal page missing"

docker stop -t 30 "$name" >/dev/null
code=$(docker inspect -f '{{.State.ExitCode}}' "$name")
[ "$code" = 0 ] || fail "exit code $code after SIGTERM"
echo "smoke: ok ($image)"
