# Build and validate packages

For installation, use the [Debian](../users/debian.md) or [TV](../users/webos.md) guide.
Building does not install or deploy a package.

## Debian arm64

On Debian arm64, install Rust, Node.js 24+/npm and the native capture dependencies:

```sh
sudo apt install build-essential pkg-config clang libclang-dev libudev-dev dpkg-dev
cargo install cargo-deb --version 3.8.0 --locked
cargo xtask setup host
cargo xtask package deb
```

The task checks generated API freshness, builds the release executable, then writes
`target/debian/`. Metadata lives in [apps/prysm/Cargo.toml](../../apps/prysm/Cargo.toml);
maintainer scripts, service and seed configuration live in
[packaging/debian](../../packaging/debian). The installed package includes a
self-contained [installation handout](../../packaging/debian/README.md).
Preserve configuration ownership, atomic writes, upgrade behavior and disabled-by-default service setup.

```sh
python3 -m unittest discover -s packaging/debian/tests
```

Run `cargo xtask check` separately for tests. CI tests Rust on arm64 and also
installs, upgrades, removes and purges the package to check configuration lifecycle.

## webOS

Pushes and manual runs of [Check and package](../../.github/workflows/check.yml)
publish `prysm-webos-armv7` under the run's **Artifacts** section. It contains
the `.ipk` built with software H.264, using the checksum-verified `2026.08-webos`
SDK. Downloads expire after three days; pull requests build and validate the package
without uploading it. Hardware H.264 remains an opt-in local build.

Install the [webOS CLI](https://github.com/webos-tools/cli), which supplies
`ares-package`, and the relocated TV SDK described in the
[capturer README](../../apps/prysm-webos-capture/README.md). From the repo root:

```sh
npm install --global @webos-tools/cli@3.2.6
cargo xtask setup
WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
  cargo xtask package webos
```

The default package uses software H.264 and has no GStreamer library dependency.
To include the experimental hardware encoder, pass `--hardware-h264` to the
same command. This enables the feature in both the worker and supervisor. The
SDK must provide GStreamer core, app, video and allocators development libraries;
the script selects the SDK's target pkg-config metadata rather than host libraries.
The checked SDK supplies GStreamer 1.20.4 and GLib 2.72.3. Hardware packages
require compatible GStreamer libraries on the TV even when software compression
is selected; missing shared libraries prevent the worker from starting.
The IPK includes MIT notices for the GStreamer Rust and GLib bindings. The native
GStreamer/GLib libraries are supplied by the TV and dynamically linked, not copied
into the package.

The task requires Node.js 24+/npm, Python 3.9+ and `ar`, checks the generated TV
service contract, typechecks and bundles the TypeScript app with Vite, builds both
binaries in one Cargo invocation, includes license notices, and creates
`target/webos-ipk/` (filename `io.prysm.capture_VERSION_all.ipk`). The version comes from
[the TV app manifest](../../crates/web/prysm-webos/public/appinfo.json). The package suffix follows the
webOS app packager convention; both executables are ARMv7 soft-float.
The final archive has root-owned files/directories with no group/other write
permissions. The packaging check rejects install scripts, links, special files,
and payload paths outside the Prysm app/service/package directories. The checker normalizes the CLI's archive ownership and modes before installation.

The CLI may still print its generic Node-version warning whenever any service
is packaged. This IPK contains no Node service files or `package.json`; that
warning does not describe the native runtime.

Use the [TV setup guide](../users/webos.md) to install with a configured ares device. For root-shell installation, replace `VERSION` with the built package version. Copy the IPK to
`/tmp` on the TV and install from a root shell:

```sh
luna-send -w 15000 -i -f luna://com.webos.appInstallService/dev/install \
  '{"id":"io.prysm.capture","ipkUrl":"/tmp/io.prysm.capture_VERSION_all.ipk","subscribe":true}'
```

Run this installation command in an interactive SSH terminal (`ssh -t`). This
TV's `luna-send` can exit before delivering a response when stdin is closed.
The package setup helper keeps a pipe open and validates the activation reply,
so boot setup and Homebrew Channel execution do not require a terminal.

## TV package verification

```sh
npm ci --prefix crates/web/prysm-webos
npm test --prefix crates/web/prysm-webos
node --test packaging/webos/tests/*.test.js
python3 -m unittest discover -s packaging/webos/tests
cargo test --locked -p prysm-webos-service --all-features
cargo test --locked -p prysm-webos-capture -p prysm-webos-protocol -p prysm-capture
```

These checks do not execute LG's native drivers. Use the
[isolated hardware tests](debugging.md#hardware-checks) separately.
