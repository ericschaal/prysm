#!/usr/bin/env bash
set -euo pipefail
sdk=/Users/eschaal/webos-sdk/arm-webos-linux-gnueabi_sdk-buildroot
export PKG_CONFIG_SYSROOT_DIR="$sdk/arm-webos-linux-gnueabi/sysroot"
export PKG_CONFIG_LIBDIR="$PKG_CONFIG_SYSROOT_DIR/usr/lib/pkgconfig:$PKG_CONFIG_SYSROOT_DIR/usr/share/pkgconfig"
export PKG_CONFIG_PATH=
read -r -a flags <<< "$("$sdk/bin/pkg-config" --cflags --libs gstreamer-app-1.0 gstreamer-video-1.0 gstreamer-allocators-1.0)"
"$sdk/bin/arm-webos-linux-gnueabi-gcc" --sysroot="$PKG_CONFIG_SYSROOT_DIR" -O2 -Wall -Wextra -Werror -Wno-misleading-indentation -mfloat-abi=softfp -mfpu=neon work/webos-encoder-latency/live/probe.c -o work/webos-encoder-latency/live/probe "${flags[@]}" -lglibc_polyfills -ldl -lc
