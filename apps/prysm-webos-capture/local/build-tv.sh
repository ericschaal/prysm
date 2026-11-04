#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
sdk=${WEBOS_SDK:?Set WEBOS_SDK to the relocated webOS SDK}
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_LINKER="$PWD/packaging/webos/linker.sh"
export CC_armv7_unknown_linux_gnueabi="$sdk/bin/arm-webos-linux-gnueabi-gcc"
export CXX_armv7_unknown_linux_gnueabi="$sdk/bin/arm-webos-linux-gnueabi-g++"
export AR_armv7_unknown_linux_gnueabi="$sdk/bin/arm-webos-linux-gnueabi-ar"
export CC_SHELL_ESCAPED_FLAGS=1
export CXXFLAGS_armv7_unknown_linux_gnueabi="--sysroot=\"$sdk/arm-webos-linux-gnueabi/sysroot\" -mfloat-abi=softfp -mfpu=neon"
export CFLAGS_armv7_unknown_linux_gnueabi="$CXXFLAGS_armv7_unknown_linux_gnueabi"
# Keep the TV's soft calling convention while doing pixel math in hardware.
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_RUSTFLAGS="-C target-cpu=cortex-a9 -C target-feature=-soft-float,+neon"
cargo build --locked -p prysm-webos-local --target armv7-unknown-linux-gnueabi --profile webos
