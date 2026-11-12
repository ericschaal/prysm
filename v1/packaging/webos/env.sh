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
# The target is soft-float ABI (as the TV's libraries are) but the TV has VFP
# and NEON: without these features Rust emulates every float operation in
# software (measured on the TV: the full worker managed 3 fps at 640x360 with a
# core pegged, against 30 fps at about 42% of one core with them). rustc warns
# that the target features are unstable; that is expected. The calling
# convention stays soft-float, which ipk.py's ELF check verifies.
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_RUSTFLAGS="-C target-cpu=cortex-a9 -C target-feature=-soft-float,+neon"
export CC_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-gcc"
export CXX_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-g++"
export AR_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-ar"
# The sysroot path may contain spaces; the cc crate then parses quoted flags.
export CC_SHELL_ESCAPED_FLAGS=1
export CFLAGS_armv7_unknown_linux_gnueabi="--sysroot=\"$webos_sysroot\" -mfloat-abi=softfp -mfpu=neon"
export CXXFLAGS_armv7_unknown_linux_gnueabi="$CFLAGS_armv7_unknown_linux_gnueabi"
# The slim worker's hardware H.264 encoder links the sysroot's GStreamer 1.0 and
# GLib (gstreamer-rs's sys crates find them with pkg-config).
export PKG_CONFIG_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/pkg-config"
export PKG_CONFIG_SYSROOT_DIR_armv7_unknown_linux_gnueabi="$webos_sysroot"
export PKG_CONFIG_LIBDIR_armv7_unknown_linux_gnueabi="$webos_sysroot/usr/lib/pkgconfig:$webos_sysroot/usr/share/pkgconfig"
export PKG_CONFIG_PATH_armv7_unknown_linux_gnueabi=
unset webos_sysroot
