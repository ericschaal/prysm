#!/usr/bin/env bash
set -euo pipefail
umask 022
if [ "$#" -ne 0 ]; then
  echo 'Usage: build-ipk.sh' >&2
  exit 1
fi
if ! command -v ares-package >/dev/null; then
  echo 'Install the webOS CLI (ares-package) before packaging.' >&2
  exit 1
fi
sdk=${WEBOS_SDK:?Set WEBOS_SDK to the relocated buildroot-nc4 SDK directory}
sdk=$(cd "$sdk" && pwd)
export WEBOS_SDK="$sdk"
root=$(cd "$(dirname "$0")/../.." && pwd)
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_LINKER="$root/packaging/webos/linker.sh"
export CC_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-gcc"
export CXX_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-g++"
export AR_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-ar"
export CC_SHELL_ESCAPED_FLAGS=1
export CXXFLAGS_armv7_unknown_linux_gnueabi="--sysroot=\"$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot\" -mfloat-abi=softfp -mfpu=neon"
export CFLAGS_armv7_unknown_linux_gnueabi="$CXXFLAGS_armv7_unknown_linux_gnueabi"
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_RUSTFLAGS="-C target-cpu=cortex-a9"
cd "$root"
cargo build --locked -p prysm-webos-capture -p prysm-webos-service \
  --target armv7-unknown-linux-gnueabi --profile webos --target-dir "$root/target"
output="$root/target/webos-ipk"
mkdir -p "$output"
build=$(mktemp -d "$output/stage.XXXXXX")
trap 'rm -rf "$build"' EXIT
mkdir -p "$build/app" "$build/service/licenses"
cp -R "$root/packaging/webos/app/." "$build/app/"
cp -R "$root/packaging/webos/service/." "$build/service/"
for binary in prysm-webos-capture prysm-webos-service; do
  cp "$root/target/armv7-unknown-linux-gnueabi/webos/$binary" "$build/service/"
done
cp "$root/LICENSE" "$build/service/licenses/prysm.txt"
for notice in hyperion tv-native-apis openh264; do
  cp "$root/crates/capture/webos-capture/LICENSE.$notice" "$build/service/licenses/$notice.txt"
done
chmod +x "$build/service/setup.sh"
chmod -R go-w "$build"
ares-package "$build/app" "$build/service" -o "$output"
# Normalize root ownership and permissions, and reject unexpected payload files.
version=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["version"])' "$root/packaging/webos/app/appinfo.json")
python3 "$root/packaging/webos/secure-ipk.py" "$output/io.prysm.capture_${version}_all.ipk"
