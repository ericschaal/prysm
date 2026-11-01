#!/usr/bin/env bash
set -euo pipefail
exec "${WEBOS_SDK:?Set WEBOS_SDK}/bin/arm-webos-linux-gnueabi-gcc" \
    --sysroot="$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot" \
    -mfloat-abi=softfp -mfpu=neon "$@" -lglibc_polyfills -lc
