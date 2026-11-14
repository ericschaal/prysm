# Build packages

| Package | Built by | Output |
| --- | --- | --- |
| Docker image `prysm-headless` (amd64, arm64) | [Dockerfile](../../packaging/docker/Dockerfile) | Local image |
| Debian package `prysm-headless` | Same Dockerfile, `deb` target | `prysm-headless_<version>_<arch>.deb` |
| TV packages, full and slim | [build-ipk.sh](../../packaging/webos/build-ipk.sh) | `io.prysm.tv-full_<version>_all.ipk`, `io.prysm.tv-slim_<version>_all.ipk` |

Reference: [packaging](../../packaging/README.md). Builds never install
or deploy. Release builds: [releasing](releasing.md).

## Docker

```sh
docker buildx build -f packaging/docker/Dockerfile -t prysm-headless:local --load .
packaging/docker/smoke.sh prysm-headless:local
```

Add `--platform linux/arm64` for a Raspberry Pi. The image runs as user 10001
(in group `video`), keeps settings in `/data`, exposes 8787 (the portal, which
also receives a slim TV's picture), and has no FFmpeg. The smoke test starts the image with a
`null` output, waits for the health check, reads the state and checks a clean
SIGTERM exit; it needs no hardware. Examples:
[compose.yaml](../../packaging/docker/compose.yaml),
[compose.truenas.yaml](../../packaging/docker/compose.truenas.yaml).
User guides: [Docker](../users/docker.md), [TrueNAS](../users/truenas.md).

<a id="debian-arm64"></a>

## Debian package

```sh
docker buildx build -f packaging/docker/Dockerfile --target deb --output type=local,dest=dist .
```

This builds for the build machine's architecture; add `--platform linux/arm64`
for a Pi. [build-deb.sh](../../packaging/debian/build-deb.sh) does the
assembly with `dpkg-deb` and also runs on any Debian or Ubuntu with
`dpkg-dev`. The binary is built on Debian 12 and needs glibc 2.34+ and systemd.

The package installs `/usr/bin/prysm-headless`, a sandboxed
[systemd unit](../../packaging/debian/prysm-headless.service) and a
sysusers entry. Installing enables and starts the service; settings live in
`/var/lib/prysm`. User guide: [Debian](../users/debian.md).

## webOS

Requirements: the relocated webOS SDK
([buildroot-nc4](https://github.com/openlgtv/buildroot-nc4)),
`rustup target add armv7-unknown-linux-gnueabi`, `ares-package` from
`npm install --global @webos-tools/cli@3.2.6`, Python 3.9+ and Node.js 24+.

```sh
npm --prefix frontend ci
npm --prefix frontend run build
export WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot
packaging/webos/build-ipk.sh full
packaging/webos/build-ipk.sh slim
```

Outputs: `target/webos-ipk/full/io.prysm.tv-full_<version>_all.ipk` and
`target/webos-ipk/slim/io.prysm.tv-slim_<version>_all.ipk`, the names the
release attaches. Both install as the same app (`io.prysm.tv`); the binaries
are ARMv7 despite `_all`.

| Package | Supervisor | Worker | TV app |
| --- | --- | --- | --- |
| `full` | `prysm-webos-service` | `prysm-webos` | `dist/webos/tv-ui` |
| `slim` | `prysm-webos-slim-service` | `prysm-webos-slim` | `dist/webos-slim/tv-ui` |

What the script enforces:

- Binaries are built with `--profile webos` for Cortex-A9 with hardware
  floating point and NEON, keeping the TV libraries' soft-float calling
  convention. Without these flags the full worker managed 3 fps at 640 × 360.
- [`ipk.py elf`](../../packaging/webos/ipk.py) refuses binaries that are
  not ARM EABI5 soft-float, are unstripped, or need a library outside the
  TV's base set or a symbol newer than glibc 2.12 or `CXXABI_1.3.9`. Only the
  slim worker may link the TV's GStreamer and GLib (hardware H.264).
- The archive is rewritten with root ownership and fixed permissions, and no
  install scripts, links, absolute or `..` paths, or files outside the app and
  service directories.

**Version.** The only source is `version` under `[workspace.package]` in
[`Cargo.toml`](../../Cargo.toml). Binaries report it in full (for
example `1.0.0-alpha.0`); `appinfo.json`, the IPK file name and its control
file carry only `major.minor.patch`, because `ares-package` accepts nothing
else.

**Licences.** Each package carries Prysm's, hyperion-webos's and the TV
native API notices, plus `javascript.txt`, which the frontend build writes
from the packages that end up in that host's bundles; the build fails if one
has no licence file. The slim package adds OpenH264, gstreamer-rs and gtk-rs.

Install on a TV: [LG TV](../users/webos.md#1-install).

## TV package checks

Without a TV:

```sh
python3 -m unittest discover -s packaging/webos/tests
python3 packaging/webos/ipk.py verify target/webos-ipk/full/io.prysm.tv-full_*_all.ipk
```

With the SDK, check the Luna and GLib bindings against its headers:

```sh
WEBOS_SDK=/path/to/sdk cargo run -p prysm-webos-luna-sys \
  --features codegen --bin generate-luna-bindings -- --check
```

On a TV: [hardware checks](debugging.md#hardware-checks).
