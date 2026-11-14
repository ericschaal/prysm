#!/usr/bin/env bash
# Cross-build the probe with the webOS SDK. Output goes to $1 (default: ./probe).
set -euo pipefail
sdk=${WEBOS_SDK:-/Users/eschaal/webos-sdk/arm-webos-linux-gnueabi_sdk-buildroot}
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-$here/probe}
"$sdk/bin/arm-webos-linux-gnueabi-gcc" --sysroot="$sdk/arm-webos-linux-gnueabi/sysroot" \
  -O2 -Wall -Wextra -Werror -mfloat-abi=softfp -mfpu=neon \
  "$here/probe.c" -o "$out" -pthread -lglibc_polyfills -ldl -lc
