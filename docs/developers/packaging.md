# Build packages

No official release yet. Build locally or use
[CI artifacts](../../.github/workflows/check.yml). Builds do not install/deploy.

## Docker

```sh
docker build -t prysm:local .
python3 packaging/docker/smoke.py prysm:local
```

Docker supplies Rust/Node and dependencies. Runtime is non-root with empty
`/config` storage; it never changes mount ownership. Smoke tests cover setup,
assets, persistence, permissions and shutdown without hardware.
Update pinned base tags/digests together; rebuild with `--pull --no-cache` to
refresh runtime packages. See [Docker setup](../users/docker.md).

## Debian arm64

On Debian arm64, with Rust and Node.js 24+/npm:

```sh
sudo apt install build-essential pkg-config clang libclang-dev libudev-dev dpkg-dev
cargo install cargo-deb --version 3.8.0 --locked
cargo xtask setup host
cargo xtask package deb
python3 -m unittest discover -s packaging/debian/tests
```

Output: `target/debian/`. [Package metadata](../../apps/prysm/Cargo.toml) and
[maintainer scripts](../../packaging/debian/) define installation. The service
starts disabled; browser saves create `/etc/prysm/settings.json` atomically.
The [included handout](../../packaging/debian/README.md) must stay self-contained.

## webOS

Requires Node.js 24+/npm, Python 3.9+, `ar` and a relocated
[buildroot-nc4 SDK](https://github.com/openlgtv/buildroot-nc4).

```sh
npm install --global @webos-tools/cli@3.2.6
rustup target add armv7-unknown-linux-gnueabi
cargo xtask setup
WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
  cargo xtask package webos
```

Output: `target/webos-ipk/io.prysm.capture_VERSION_all.ipk`; both binaries are
ARMv7 soft-float despite `_all`. The task checks contracts, bundles the TV UI and
packages both binaries/notices. Keep archive path/permission validation enabled.
[Worker ABI requirements](../../apps/prysm-webos-capture/README.md#build-for-webos).

Add `--hardware-h264` for the experimental encoder. This requires SDK development
files and TV runtime libraries for GStreamer/GLib, even when software encoding is
selected. [Encoder details](hardware-h264.md).

Push/manual CI artifacts last three days; PR builds do not upload. Install using
[TV setup](../users/webos.md).

## TV package verification

```sh
npm test --prefix crates/web/prysm-webos
node --test packaging/webos/tests/*.test.js
python3 -m unittest discover -s packaging/webos/tests
cargo test --locked -p prysm-webos-service --all-features
cargo test --locked -p prysm-webos-capture -p prysm-webos-protocol -p prysm-capture
```

Then run [hardware checks](debugging.md#hardware-checks) on the target TV.
