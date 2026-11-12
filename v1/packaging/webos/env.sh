# Cross-build environment for the TV binaries. Source it with WEBOS_SDK set:
#   WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot
#   . v1/packaging/webos/env.sh
#   cargo build --manifest-path v1/Cargo.toml --target armv7-unknown-linux-gnueabi --profile webos -p prysm-webos
# shellcheck shell=bash
if [ -z "${WEBOS_SDK:-}" ] || [ ! -x "$WEBOS_SDK/bin/arm-webos-linux-gnueabi-gcc" ]; then
  echo 'Set WEBOS_SDK to the relocated webOS SDK (arm-webos-linux-gnueabi_sdk-buildroot).' >&2
  return 1
fi
WEBOS_SDK=$(cd "$WEBOS_SDK" && pwd)
export WEBOS_SDK
webos_sysroot="$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot"
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_LINKER="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/linker.sh"
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_LINKER
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_RUSTFLAGS="-C target-cpu=cortex-a9"
export CC_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-gcc"
export CXX_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-g++"
export AR_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-ar"
# The sysroot path may contain spaces; the cc crate then parses quoted flags.
export CC_SHELL_ESCAPED_FLAGS=1
export CFLAGS_armv7_unknown_linux_gnueabi="--sysroot=\"$webos_sysroot\" -mfloat-abi=softfp -mfpu=neon"
export CXXFLAGS_armv7_unknown_linux_gnueabi="$CFLAGS_armv7_unknown_linux_gnueabi"
unset webos_sysroot
