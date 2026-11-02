#!/bin/sh
set -eu
sdk=/Users/eschaal/webos-sdk/arm-webos-linux-gnueabi_sdk-buildroot
"$sdk/bin/arm-webos-linux-gnueabi-g++" --sysroot="$sdk/arm-webos-linux-gnueabi/sysroot" -std=c++11 -O2 -Wall -Wextra -Werror -mfloat-abi=softfp -mfpu=neon -mcpu=cortex-a9 -I crates/capture/webos-capture/native work/webos-osd-home/retry/probe.cpp -o work/webos-osd-home/retry/probe -ldl -pthread
