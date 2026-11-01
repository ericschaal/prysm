#!/usr/bin/env bash
set -euo pipefail
sdk=${WEBOS_SDK:?Set WEBOS_SDK to the relocated buildroot-nc4 SDK directory}
export WEBOS_SDK="$(cd "$sdk" && pwd)"
root=$(cd "$(dirname "$0")/../../.." && pwd)
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_LINKER="$root/crates/capture/webos-capture/webos-linker.sh"
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_RUSTFLAGS="-C target-cpu=cortex-a9"
cd "$root"
cargo build --locked -p prysm-webos-service --bin prysm-webos-service \
  --target armv7-unknown-linux-gnueabi --profile webos
