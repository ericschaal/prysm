#!/usr/bin/env bash
set -euo pipefail
umask 022
features=(--no-default-features)
case "$#:${1-}" in
  0:) ;;
  1:--hardware-h264)
    features+=(--features prysm-webos-capture/hardware-h264,prysm-webos-service/hardware-h264)
    ;;
  *) echo 'Usage: build-ipk.sh [--hardware-h264]' >&2; exit 1 ;;
esac
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
export PKG_CONFIG_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/pkg-config"
export PKG_CONFIG_SYSROOT_DIR_armv7_unknown_linux_gnueabi="$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot"
export PKG_CONFIG_LIBDIR_armv7_unknown_linux_gnueabi="$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot/usr/lib/pkgconfig:$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot/usr/share/pkgconfig"
export PKG_CONFIG_PATH_armv7_unknown_linux_gnueabi=
cd "$root"
cargo build --locked "${features[@]}" -p prysm-webos-capture -p prysm-webos-service \
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
for notice in hyperion tv-native-apis openh264 gstreamer-rs gtk-rs; do
  cp "$root/crates/capture/webos-capture/LICENSE.$notice" "$build/service/licenses/$notice.txt"
done
chmod +x "$build/service/setup.sh"
chmod -R go-w "$build"
ares-package "$build/app" "$build/service" -o "$output"
# Normalize root ownership and permissions, and reject unexpected payload files.
version=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["version"])' "$root/packaging/webos/app/appinfo.json")
python3 "$root/packaging/webos/secure-ipk.py" "$output/io.prysm.capture_${version}_all.ipk"
