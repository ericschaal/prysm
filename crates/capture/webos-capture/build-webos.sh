#!/usr/bin/env bash
set -euo pipefail

# Use the TV SDK's soft-float ABI and sysroot, never the host's Linux linker.
sdk=${WEBOS_SDK:?Set WEBOS_SDK to the relocated buildroot-nc4 SDK directory}
sdk=$(cd "$sdk" && pwd)
export WEBOS_SDK="$sdk"
root=$(cd "$(dirname "$0")/../../.." && pwd)
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_LINKER="$root/crates/capture/webos-capture/webos-linker.sh"
export CC_armv7_unknown_linux_gnueabi="$sdk/bin/arm-webos-linux-gnueabi-gcc"
export CXX_armv7_unknown_linux_gnueabi="$sdk/bin/arm-webos-linux-gnueabi-g++"
export AR_armv7_unknown_linux_gnueabi="$sdk/bin/arm-webos-linux-gnueabi-ar"
export CC_SHELL_ESCAPED_FLAGS=1
export CXXFLAGS_armv7_unknown_linux_gnueabi="--sysroot=\"$sdk/arm-webos-linux-gnueabi/sysroot\" -mfloat-abi=softfp -mfpu=neon"
export CFLAGS_armv7_unknown_linux_gnueabi="$CXXFLAGS_armv7_unknown_linux_gnueabi"
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_RUSTFLAGS="-C target-cpu=cortex-a9"
cd "$root"
cargo build --locked -p prysm-webos-capture --target armv7-unknown-linux-gnueabi --profile webos --target-dir "$root/target"

dist="$root/crates/capture/webos-capture/dist"
mkdir -p "$dist/licenses"
cp "$root/target/armv7-unknown-linux-gnueabi/webos/prysm-webos-capture" "$dist/"
cp -R "$root/crates/capture/webos-capture/permissions" "$dist/"
cp "$root/crates/capture/webos-capture/LICENSE.hyperion" "$dist/licenses/hyperion-webos.txt"
cp "$root/crates/capture/webos-capture/LICENSE.tv-native-apis" "$dist/licenses/tv-native-apis.txt"
cp "$root/crates/capture/webos-capture/LICENSE.openh264" "$dist/licenses/openh264.txt"
printf 'Built %s\n' "$dist/prysm-webos-capture"
