# Build packages

Builds produce local artifacts; they do not install or deploy them. Prysm has no
official release yet. Use local builds or artifacts from
[Check and package](../../.github/workflows/check.yml).

## Docker

```sh
docker build -t prysm:local .
python3 packaging/docker/smoke.py prysm:local
```

`cargo xtask package docker` runs the same build. Docker supplies Rust, Node,
locked dependencies and contract checks; host Node and the TV SDK are unnecessary.
The runtime contains the embedded panel, required libraries and license notices.
It runs as a non-root user, with `PRYSM_CONFIG=/config/settings.json` and empty
settings storage. It does not change mounted directory ownership.

The smoke test checks setup, assets, process-option precedence, settings validation,
persistence, UID 568 storage, write failures, malformed files and SIGTERM. It
cleans temporary resources and needs no capture/WLED hardware.

Base images are pinned by digest. Update tags and digests together; use
`--pull --no-cache` when refreshing runtime packages, then repeat the smoke test.
CI checks native amd64/arm64 images. For mounting storage and device access, see
[Docker setup](../users/docker.md) or [TrueNAS](../users/truenas.md).

## Debian arm64

On Debian arm64, install Rust, Node.js 24+/npm and:

```sh
sudo apt install build-essential pkg-config clang libclang-dev libudev-dev dpkg-dev
cargo install cargo-deb --version 3.8.0 --locked
cargo xtask setup host
cargo xtask package deb
```

The task checks the host contract and builds the executable and `.deb` in
`target/debian/`. Metadata lives in
[apps/prysm/Cargo.toml](../../apps/prysm/Cargo.toml); service and maintainer scripts
live in [packaging/debian](../../packaging/debian).

The package includes a [self-contained handout](../../packaging/debian/README.md).
Its service starts disabled. Installation creates writable `/etc/prysm`, owned by
the service user; the first browser save creates `settings.json`. Preserve atomic
writes and the documented remove/purge behavior.

```sh
python3 -m unittest discover -s packaging/debian/tests
```

CI also checks installation and configuration lifecycle. Run normal Rust/frontend
checks separately; packaging is not a substitute for them.

## webOS

Install Node.js 24+/npm, Python 3.9+, `ar`, the
[webOS CLI](https://github.com/webos-tools/cli), and a relocated
[buildroot-nc4 SDK](https://github.com/openlgtv/buildroot-nc4):

```sh
npm install --global @webos-tools/cli@3.2.6
rustup target add armv7-unknown-linux-gnueabi
cargo xtask setup
WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
  cargo xtask package webos
```

The task checks the TV contract, builds its frontend and both Rust binaries,
then writes `target/webos-ipk/io.prysm.capture_VERSION_all.ipk`. The version comes
from [appinfo.json](../../crates/web/prysm-webos/public/appinfo.json).
Despite the package's `_all` suffix, the executables are ARMv7 soft-float.
The SDK compiler/linker uses Cortex-A9, the target sysroot and glibc polyfills.
See the [worker build contract](../../apps/prysm-webos-capture/README.md#build-for-webos).

The default encoder is software H.264. Add `--hardware-h264` to enable the
experimental backend in both binaries. The SDK must provide GStreamer core,
app, video, allocators and GLib development files. The TV must also provide those
shared libraries, even when software encoding is selected in a hardware-enabled
package. See [hardware H.264](hardware-h264.md).

Package verification enforces root-owned files, restrictive permissions and
payload paths limited to Prysm's app/service/package directories. Links, special
files and install scripts are rejected. License notices are included. The CLI's
generic Node-service warning does not apply: the native runtime needs no Node.

Push/manual CI runs upload `prysm-webos-armv7` artifacts for three days. Pull
requests build/check without uploading. The CI SDK is checksum-verified.
Install using the [TV setup guide](../users/webos.md).

## TV package verification

```sh
npm test --prefix crates/web/prysm-webos
node --test packaging/webos/tests/*.test.js
python3 -m unittest discover -s packaging/webos/tests
cargo test --locked -p prysm-webos-service --all-features
cargo test --locked -p prysm-webos-capture -p prysm-webos-protocol -p prysm-capture
```

These tests do not run LG drivers. Follow the [hardware checks](debugging.md#hardware-checks)
for a target TV.
