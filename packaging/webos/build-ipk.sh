#!/usr/bin/env bash
set -euo pipefail
umask 022
features=(--no-default-features
  --features "prysm-webos-capture/hardware-h264,prysm-webos-service/hardware-h264")
embedded=0
for option in "$@"; do
  case "$option" in
    --embedded-prysm)
      embedded=1
      features+=(--features "prysm-webos-service/embedded-prysm")
      ;;
    *) echo 'Usage: build-ipk.sh [--embedded-prysm]' >&2; exit 1 ;;
  esac
done
if ! command -v ares-package >/dev/null; then
  echo 'Install the webOS CLI (ares-package) before packaging.' >&2
  exit 1
fi
sdk=${WEBOS_SDK:?Set WEBOS_SDK to the relocated buildroot-nc4 SDK directory}
sdk=$(cd "$sdk" && pwd)
export WEBOS_SDK="$sdk"
root=$(cd "$(dirname "$0")/../.." && pwd)
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_LINKER="$root/packaging/webos/linker.sh"
export CC_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-gcc"
export CXX_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-g++"
export AR_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-ar"
export CC_SHELL_ESCAPED_FLAGS=1
export CXXFLAGS_armv7_unknown_linux_gnueabi="--sysroot=\"$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot\" -mfloat-abi=softfp -mfpu=neon"
export CFLAGS_armv7_unknown_linux_gnueabi="$CXXFLAGS_armv7_unknown_linux_gnueabi"
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_RUSTFLAGS="-C target-cpu=cortex-a9"
export PKG_CONFIG_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/pkg-config"
export PKG_CONFIG_SYSROOT_DIR_armv7_unknown_linux_gnueabi="$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot"
export PKG_CONFIG_LIBDIR_armv7_unknown_linux_gnueabi="$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot/usr/lib/pkgconfig:$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot/usr/share/pkgconfig"
export PKG_CONFIG_PATH_armv7_unknown_linux_gnueabi=
cd "$root"
# Cargo's top-level executable names are shared across feature combinations.
cargo_target="$root/target"
if ((embedded)); then
  cargo_target="$root/target/webos-build/embedded"
fi
cargo build --locked "${features[@]}" -p prysm-webos-capture -p prysm-webos-service \
  --target armv7-unknown-linux-gnueabi --profile webos --target-dir "$cargo_target"
# Build the isolated smoke fixture beside the worker; only the worker ships in the IPK.
cargo build --locked -p prysm-webos-capture --example loopback-receiver \
  --target armv7-unknown-linux-gnueabi --profile webos --target-dir "$cargo_target"
if ((embedded)); then
  # Build separately so the remote sender's transport/codec features cannot enter the local worker.
  CARGO_TARGET_DIR="$cargo_target" "$root/apps/prysm-webos-capture/local/build-tv.sh" --embedded-prysm
fi
output="$root/target/webos-ipk"
if ((embedded)); then output="$output/embedded"; fi
mkdir -p "$output"
build=$(mktemp -d "$output/stage.XXXXXX")
trap 'rm -rf "$build"' EXIT
mkdir -p "$build/app" "$build/service/licenses"
VITE_EMBEDDED_PRYSM="$embedded" npm run build --prefix "$root/crates/web/prysm-webos" -- \
  --outDir "$build/app" --emptyOutDir
cp -R "$root/packaging/webos/service/." "$build/service/"
for binary in prysm-webos-capture prysm-webos-service; do
  cp "$cargo_target/armv7-unknown-linux-gnueabi/webos/$binary" "$build/service/"
done
if ((embedded)); then
  cp "$cargo_target/armv7-unknown-linux-gnueabi/webos/prysm-webos-local" "$build/service/"
  python3 - "$build/service" <<'PYROLE'
import json
from pathlib import Path
import sys
service = Path(sys.argv[1])
role = json.loads((service / "capture.role.json").read_text())
role["exeName"] = str(Path(role["exeName"]).with_name("prysm-webos-local"))
(service / "embedded.role.json").write_text(json.dumps(role, indent=2) + "\n")
PYROLE
fi
cp "$root/LICENSE" "$build/service/licenses/prysm.txt"
for notice in hyperion tv-native-apis openh264 gstreamer-rs gtk-rs; do
  cp "$root/apps/prysm-webos-capture/LICENSE.$notice" "$build/service/licenses/$notice.txt"
done
chmod +x "$build/service/setup.sh"
chmod -R go-w "$build"
ares-package "$build/app" "$build/service" -o "$build"
# Normalize root ownership and permissions, and reject unexpected payload files.
version=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["version"])' "$root/crates/web/prysm-webos/public/appinfo.json")
package="io.prysm.capture_${version}_all.ipk"
python3 "$root/packaging/webos/secure-ipk.py" "$build/$package" "$embedded"
mv "$build/$package" "$output/$package"
printf '%s\n' "$output/$package"
