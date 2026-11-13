#!/usr/bin/env bash
# Build one v1 TV package: io.prysm.tv-<flavour>_<version>_all.ipk, full or slim
# flavour. Both install as app io.prysm.tv with service io.prysm.tv.service,
# so installing one replaces the other. Builds do not install or deploy.
#
#   WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
#     v1/packaging/webos/build-ipk.sh full|slim [--frontend] [--out DIR]
#
# The TV UI bundle (and, for full, the portal assets the worker embeds) is an
# input from `npm --prefix v1/frontend run build`; --frontend runs that first.
set -euo pipefail
umask 022

usage() {
  echo 'Usage: build-ipk.sh full|slim [--frontend] [--out DIR]' >&2
  exit 2
}

flavour=${1:-}
case "$flavour" in
  full) supervisor=prysm-webos-service worker=prysm-webos host=webos ;;
  # The slim worker encodes with the TV's hardware H.264 encoder (GStreamer)
  # or OpenH264, so it links the TV's GStreamer and GLib and ships their notices.
  slim) supervisor=prysm-webos-slim-service worker=prysm-webos-slim host=webos-slim ;;
  *) usage ;;
esac
shift
frontend=0
out=
while (($#)); do
  case "$1" in
    --frontend) frontend=1 ;;
    --out) [ $# -ge 2 ] || usage; out=$2; shift ;;
    *) usage ;;
  esac
  shift
done

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
v1=$(cd "$here/../.." && pwd)
root=$(cd "$v1/.." && pwd)
# shellcheck source=env.sh
. "$here/env.sh"
for tool in ares-package python3 cargo; do
  command -v "$tool" >/dev/null || { echo "build-ipk.sh: $tool is required" >&2; exit 1; }
done

if ((frontend)); then
  npm --prefix "$v1/frontend" run build
fi
ui="$v1/frontend/dist/$host/tv-ui"
[ -f "$ui/index.html" ] || {
  echo "build-ipk.sh: no TV UI bundle at $ui; run npm --prefix v1/frontend run build (or pass --frontend)" >&2
  exit 1
}
if [ "$flavour" = full ] && [ ! -f "$v1/frontend/dist/webos/web-portal/assets.rs" ]; then
  echo 'build-ipk.sh: the full worker embeds the portal; build v1/frontend first' >&2
  exit 1
fi

# Notices for the JavaScript in this host's bundles, written by the frontend build.
notices="$v1/frontend/dist/$host/licenses.txt"
[ -f "$notices" ] || {
  echo "build-ipk.sh: no JavaScript notices at $notices; rebuild v1/frontend (or pass --frontend)" >&2
  exit 1
}

target_dir=${CARGO_TARGET_DIR:-$v1/target}
cargo build --manifest-path "$v1/Cargo.toml" --offline \
  --target armv7-unknown-linux-gnueabi --profile webos --target-dir "$target_dir" \
  -p "$worker" -p prysm-webos-service --bin "$worker" --bin "$supervisor"
binaries="$target_dir/armv7-unknown-linux-gnueabi/webos"
python3 "$here/ipk.py" elf "$binaries/$supervisor"
if [ "$flavour" = slim ]; then
  python3 "$here/ipk.py" elf --gstreamer "$binaries/$worker"
else
  python3 "$here/ipk.py" elf "$binaries/$worker"
fi

output=${out:-$v1/target/webos-ipk/$flavour}
mkdir -p "$output"
output=$(cd "$output" && pwd)
stage=$(mktemp -d "$output/stage.XXXXXX")
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/app" "$stage/service/licenses"
cp -R "$ui/." "$stage/app/"
cp "$here/app/icon.png" "$stage/app/"
# The one version source is [workspace.package] in v1/Cargo.toml; appinfo.json
# takes its major.minor.patch (ares-package accepts nothing else).
version=$(python3 "$here/ipk.py" appinfo "$v1/Cargo.toml" "$here/app/appinfo.json" "$stage/app/appinfo.json")
for file in services.json setup.sh supervisor.role.json worker.role.json supervisor.service \
  supervisor.client.json supervisor.api.json; do
  cp "$here/service/$file" "$stage/service/"
done
# Installed names are the same in both flavours.
cp "$binaries/$supervisor" "$stage/service/prysm-webos-service"
cp "$binaries/$worker" "$stage/service/prysm-webos-worker"
cp "$root/LICENSE" "$stage/service/licenses/prysm.txt"
cp "$notices" "$stage/service/licenses/javascript.txt"
for notice in hyperion tv-native-apis; do
  cp "$v1/adapters/capture-webos-sys/LICENSE.$notice" "$stage/service/licenses/$notice.txt"
done
if [ "$flavour" = slim ]; then
  cp "$v1/adapters/h264/LICENSE.openh264" "$stage/service/licenses/openh264.txt"
  for notice in gstreamer-rs gtk-rs; do
    cp "$v1/adapters/h264-rvenc-sys/LICENSE.$notice" "$stage/service/licenses/$notice.txt"
  done
fi
chmod 755 "$stage/service/setup.sh" "$stage/service/prysm-webos-service" "$stage/service/prysm-webos-worker"
chmod -R go-w "$stage"

ares-package "$stage/app" "$stage/service" -o "$stage" >&2
built="io.prysm.tv_${version}_all.ipk"
# Root ownership and fixed modes; reject install scripts, links and stray paths.
python3 "$here/ipk.py" secure "$stage/$built"
python3 "$here/ipk.py" verify "$stage/$built" >/dev/null
# Both flavours install as io.prysm.tv; the file name says which one it is,
# and is the name the release attaches.
package="io.prysm.tv-${flavour}_${version}_all.ipk"
mv "$stage/$built" "$output/$package"
printf '%s\n' "$output/$package"
