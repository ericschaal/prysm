#!/usr/bin/env bash
set -euo pipefail
umask 022
root=$(cd "$(dirname "$0")/../.." && pwd)
native=false
case "${1:-}" in
  '') ;;
  --native) native=true ;;
  *) echo 'Usage: build-ipk.sh [--native]' >&2; exit 1 ;;
esac
output="$root/target/webos-ipk"
if "$native"; then
  bash "$root/crates/capture/webos-service/build-webos.sh"
  output="$output/native"
fi
if ! command -v ares-package >/dev/null; then
  echo 'Install the webOS CLI (ares-package) before packaging.' >&2
  exit 1
fi
"$root/crates/capture/webos-capture/build-webos.sh"
mkdir -p "$output"
build=$(mktemp -d "$root/target/webos-ipk/stage.XXXXXX")
trap 'rm -rf "$build"' EXIT
mkdir -p "$build/app" "$build/service"
cp -R "$root/packaging/webos/app/." "$build/app/"
cp -R "$root/packaging/webos/service/." "$build/service/"
cp "$root/crates/capture/webos-capture/dist/prysm-webos-capture" "$build/service/"
cp -R "$root/crates/capture/webos-capture/dist/licenses" "$build/service/"
cp "$root/LICENSE" "$build/service/licenses/prysm.txt"
if "$native"; then
  cp -R "$root/packaging/webos/native-service/." "$build/service/"
  cp "$root/target/armv7-unknown-linux-gnueabi/webos/prysm-webos-service" "$build/service/"
fi
chmod +x "$build/service/setup.sh"
chmod -R go-w "$build"
ares-package "$build/app" "$build/service" -o "$output"
# ares-package creates generated JS as 0666 and service directories as 0777.
version=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["version"])' "$root/packaging/webos/app/appinfo.json")
python3 "$root/packaging/webos/secure-ipk.py" "$output/io.prysm.capture_${version}_all.ipk"
