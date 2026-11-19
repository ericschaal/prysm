# Build packages

| Package | Built by | Output |
| --- | --- | --- |
| Docker image `prysm-headless` (amd64, arm64) | [Dockerfile](../../packaging/docker/Dockerfile) | Local image, or `prysm-headless-linux-<arch>.tar` in `dist/`, released as `ghcr.io/<owner>/prysm-headless:<version>` |
| Debian package `prysm-headless` | Same Dockerfile, `artifacts` target | `prysm-headless-linux-<arch>.deb`, released as `prysm-headless_<version>_<arch>.deb` |
| TV packages, full and slim | [build-ipk.sh](../../packaging/webos/build-ipk.sh) | `prysm-tv-full.ipk`, `prysm-tv-slim.ipk` in `dist/`, released as `io.prysm.tv-full_<version>_all.ipk`, `io.prysm.tv-slim_<version>_all.ipk` |

Reference: [packaging](../../packaging/README.md). Builds never install
or deploy. Release builds: [releasing](releasing.md).

## Artifacts in dist/

`cargo xtask dist TARGET...` writes artifacts under fixed names to `dist/`
(ignored by git) and records each in `dist/manifest.json`:

| Target | File | Built by |
| --- | --- | --- |
| `mac` | `prysm-headless-macos-arm64` (this Mac's architecture) | `npm run bundle`, then `cargo build --release --locked -p prysm-headless`; macOS only |
| `linux-arm64`, `linux-amd64` | `prysm-headless-linux-<arch>` | The Dockerfile's `artifacts` target |
| `deb-arm64`, `deb-amd64` | `prysm-headless-linux-<arch>.deb` | The same `artifacts` target |
| `image-arm64`, `image-amd64` | `prysm-headless-linux-<arch>.tar` | The Dockerfile's `runtime` target, an archive for `docker load` |
| `tv-full`, `tv-slim` | `prysm-tv-full.ipk`, `prysm-tv-slim.ipk` | `npm run bundle`, then [build-ipk.sh](../../packaging/webos/build-ipk.sh); needs `WEBOS_SDK` (see [webOS](#webos)) |

Every artifact embeds the portal: the `release` and `webos` profiles fail
without `frontend/dist`, where a debug build would fall back to a "not
built" page ([build](README.md#build-and-run)).

```sh
cargo xtask dist linux-arm64 deb-arm64 image-arm64
docker load --input dist/prysm-headless-linux-arm64.tar   # prysm-headless:local
```

The Docker targets need buildx; a target for the other architecture is
emulated and slow. The binary and the `.deb` of one architecture come from one
Docker build, and one run bundles the frontend once for the targets that take
it (`mac`, `tv-*`). The image is labelled with the commit
(`org.opencontainers.image.revision`). xtask adds the words of
`XTASK_BUILDX_ARGS`, if set, to every `docker buildx build`; CI passes its
layer cache that way. Desktop release builds keep their symbols, so a panic or
backtrace names functions; only the TV's `webos` profile strips
(`Cargo.toml`).

The manifest has one entry per file: the version from `VERSION`, the commit,
whether the tree had uncommitted changes, the SHA-256 and the build time in
UTC. Building a target replaces its entry and keeps the others; entries whose
file is no longer in `dist/` are dropped. Each target prints how long it took,
and a run from a tree with uncommitted changes ends with a warning.

CI builds its packages with the same command, so they have the same names:
the `docker` job runs `dist linux-<arch> deb-<arch> image-<arch>` on amd64 and
arm64, and `webos-ipk` runs `dist tv-full tv-slim`. Each uploads its `dist/`
with its manifest as one artifact (`prysm-dist-linux-<arch>`, `prysm-dist-tv`)
on release runs (the IPKs also on pushes to `main`), and a release checks
every file against its manifest with `cargo xtask release-artifacts`
([releasing](releasing.md#cut-a-release)).

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
cargo xtask dist deb-arm64   # or deb-amd64
```

This runs the Dockerfile's `artifacts` target for that platform and writes
`dist/prysm-headless-linux-<arch>.deb`; a release attaches it as
`prysm-headless_<version>_<arch>.deb`, with the product version.
[build-deb.sh](../../packaging/debian/build-deb.sh) does the
assembly with `dpkg-deb` and also runs on any Debian or Ubuntu with
`dpkg-dev`. The binary is built on Debian 12 and needs glibc 2.34+ and systemd.

The package installs `/usr/bin/prysm-headless` and a sandboxed
[systemd unit](../../packaging/debian/prysm-headless.service); installing
enables and starts the service. User guide: [Debian](../users/debian.md).

The unit has `DynamicUser=yes`, so the package creates no account: while the
service runs, systemd allocates a user and group named after the unit
(`prysm-headless`) and adds `video` (`SupplementaryGroups=`) for
`/dev/video*`. With a dynamic user, `StateDirectory=prysm` is
`/var/lib/private/prysm` (mode 0700; systemd gives the running user access on
each start, whatever UID it has that time), and `/var/lib/prysm` is a symlink
to it, so the unit's settings path stays `/var/lib/prysm/settings.json`.
`DynamicUser=` also implies `ProtectSystem=strict`, `ProtectHome=read-only`,
`PrivateTmp=`, `RemoveIPC=`, `NoNewPrivileges=` and `RestrictSUIDSGID=`, all
of which the unit already sets (`ProtectHome=true`, stricter); it does not
imply `PrivateDevices=`, and V4L2 access comes from `DevicePolicy=closed`
with `DeviceAllow=char-video4linux rw`. The service writes only its settings
file (a temporary file in the same directory, then a rename) and starts no
other program. `apt purge` deletes `/var/lib/private/prysm` and the
`/var/lib/prysm` link and leaves systemd's `/var/lib/private`.

## webOS

Requirements: the relocated webOS SDK
([buildroot-nc4](https://github.com/openlgtv/buildroot-nc4)),
`rustup target add armv7-unknown-linux-gnueabi`, `ares-package` from
`npm install --global @webos-tools/cli@3.2.6`, Python 3.9+ and Node.js 24+.

```sh
export WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot
cargo xtask dist tv-full tv-slim
```

This bundles the frontend and runs `build-ipk.sh` for each package, which
writes `io.prysm.tv-full_<version>_all.ipk` and
`io.prysm.tv-slim_<version>_all.ipk`, the names the release attaches; xtask
copies them to `dist/prysm-tv-full.ipk` and `dist/prysm-tv-slim.ipk`. Run
directly (after `npm --prefix frontend run bundle`),
`packaging/webos/build-ipk.sh full` writes to `target/webos-ipk/full/`. Both install as the same app (`io.prysm.tv`); the binaries
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

**Version.** The only source is [`VERSION`](../../VERSION) (see
[releasing](releasing.md#the-version-is-one-file)). Binaries report it in full
(for example `1.0.0-alpha.0`); `appinfo.json`, the IPK file name and its
control file carry only `major.minor.patch`, because `ares-package` accepts
nothing else.

**Licences.** Each package carries Prysm's, hyperion-webos's and the TV
native API notices, plus `javascript.txt`, which the frontend build writes
from the packages that end up in that host's bundles; the build fails if one
has no licence file. The slim package adds OpenH264, gstreamer-rs and gtk-rs.

Install on a TV: [LG TV](../users/webos.md#1-install). `install-ipk.sh`
stops a running Prysm through its Luna `uninstall` before the install
service would end it abruptly ([webOS](webos.md#files-on-the-tv)).

## TV package checks

Without a TV:

```sh
python3 -m unittest discover -s packaging/webos/tests   # in cargo xtask check scripts
python3 packaging/webos/ipk.py verify dist/prysm-tv-full.ipk
```

With the SDK, check the Luna and GLib bindings against its headers:

```sh
WEBOS_SDK=/path/to/sdk cargo xtask codegen --check --luna
```

Without `--check` it regenerates them.

On a TV: [hardware checks](debugging.md#hardware-checks).
