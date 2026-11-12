#!/usr/bin/env bash
# Cargo linker for armv7-unknown-linux-gnueabi against the webOS SDK sysroot.
# ARMv7 soft-float ABI with NEON; glibc_polyfills supplies symbols (getauxval)
# that the TV's glibc 2.12 lacks.
set -euo pipefail
exec "${WEBOS_SDK:?Set WEBOS_SDK to the webOS SDK (arm-webos-linux-gnueabi_sdk-buildroot)}/bin/arm-webos-linux-gnueabi-gcc" \
    --sysroot="$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot" \
    -mfloat-abi=softfp -mfpu=neon "$@" -lglibc_polyfills -ldl -lc
