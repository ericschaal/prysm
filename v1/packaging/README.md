# Packaging prysm-headless

Two ways to run the v1 [headless host](../hosts/headless/src/main.rs) on Linux:
a Docker image and a Debian package with a systemd service. Both are built by
[`docker/Dockerfile`](docker/Dockerfile) from the repository root, for
`linux/amd64` and `linux/arm64` (Raspberry Pi OS 64-bit). They are separate from
the shipping product's [image](../../Dockerfile) and
[package](../../packaging/debian/README.md), and assume a machine without them.

Both offer the `device` (V4L2) and `remote` sources and the `wled` and `null`
outputs. The `file` source needs FFmpeg 9+ on `PATH`, which neither ships.

## Portal and settings

The portal listens on port **8787** on every interface: open
`http://<machine>:8787`. It has no password, so anyone who can reach the port
can control the lights; keep it on a trusted LAN. Browser pages from other
sites are refused unless allowed with `--allow-origin`
([access](../surfaces/web-portal/HTTP-CONTRACT.md#access)).

Settings are one JSON file ([format](../adapters/settings-file/README.md),
[example](../hosts/headless/settings.example.json)). A missing file means
"unconfigured": the host starts idle and the portal writes the file on the
first save. Hosts start idle; add `--start` to request lighting at launch.

| | Docker | Debian |
| --- | --- | --- |
| Settings | `/data/settings.json` (volume) | `/var/lib/prysm/settings.json` |
| Runs as | UID/GID 10001, plus `video` (GID 44) | system user `prysm`, plus `video` |
| Logs | `docker logs` | `journalctl -u prysm-headless` |
| Extra options | arguments after the image name | `systemctl edit prysm-headless` |

`RUST_LOG` filters log lines (default `warn,prysm=info`).

## Docker

Build (BuildKit/buildx required; `--platform linux/arm64` for a Pi):

```sh
docker buildx build -f v1/packaging/docker/Dockerfile -t prysm-headless:local --load .
v1/packaging/docker/smoke.sh prysm-headless:local   # optional check
```

Run with a USB capture device:

```sh
docker run -d --name prysm --restart unless-stopped --stop-timeout 30 \
  -p 8787:8787 -v prysm-data:/data \
  --device /dev/video0 --group-add "$(getent group video | cut -d: -f3)" \
  prysm-headless:local
```

or `docker compose -f v1/packaging/docker/compose.yaml up -d`
([compose.yaml](docker/compose.yaml)). Notes:

- Arguments after the image name are appended to
  `prysm-headless /data/settings.json`, e.g. `--start` or
  `--allow-origin http://192.168.1.30:3000`.
- A bind mount instead of the `prysm-data` volume must be writable by UID
  10001 (`chown 10001:10001 DIR`).
- The `remote` source listens on the port in its settings (suggested 7380);
  publish it too (`-p 7380:7380`).
- A bridge network cannot resolve mDNS names such as `wled.local`; use the
  WLED's IP address or `--network host`.
- The image's `HEALTHCHECK` reads `GET /api/v1/state` on port 8787; it reports
  unhealthy with `--no-portal` or another `--listen` port.
- `docker stop` sends SIGTERM; the host idles the LEDs and exits. Allow 30 s.

Release images are published as `ghcr.io/ericschaal/prysm-headless:<version>`
by [v1-release.yml](../../.github/workflows/v1-release.yml) on a `v1.*` tag.

## Debian package

Build `prysm-headless_<version>_<arch>.deb` for the build machine's
architecture (any OS with Docker; add `--platform linux/arm64` for a Pi):

```sh
docker buildx build -f v1/packaging/docker/Dockerfile --target deb --output type=local,dest=dist .
```

[`debian/build-deb.sh BINARY OUT_DIR`](debian/build-deb.sh) does the
assembly with `dpkg-deb` and can run on any Debian/Ubuntu with `dpkg-dev`.
The binary is built on Debian 12 and needs systemd and glibc 2.34 or newer
(Debian 12+, Ubuntu 22.04+, Raspberry Pi OS bookworm, 64-bit).

Install and use:

```sh
sudo apt install ./prysm-headless_*.deb   # creates user prysm, enables and starts the service
systemctl status prysm-headless
journalctl -u prysm-headless -f
sudo systemctl edit prysm-headless        # e.g. ExecStart= then ExecStart=/usr/bin/prysm-headless /var/lib/prysm/settings.json --start
```

The package installs `/usr/bin/prysm-headless`, the
[unit](debian/prysm-headless.service) and a
[sysusers.d](debian/prysm-headless.sysusers) entry. The unit runs as `prysm`
with `video` for `/dev/video*`, keeps its state in `StateDirectory=prysm`,
restarts on failure and is sandboxed (read-only system, no capabilities,
only V4L2 devices, IPv4/IPv6/Unix sockets). `apt remove` stops and disables
the service; `apt purge` also deletes `/var/lib/prysm`. The `prysm` user is
kept.

## webOS TV packages

Two IPKs for a rooted LG TV ([design](../../docs/plans/prysm-v1-webos-supervisor.md)),
built by [`webos/build-ipk.sh`](webos/build-ipk.sh). Both are the app
`io.prysm.tv` with the service `io.prysm.tv.service` and the same installed
file names, so installing one replaces the other. Nothing detects or migrates
the old `io.prysm.capture` app: remove it (and its service setup) first.

| Flavour | The TV… | Supervisor (cargo bin) | Worker (cargo bin) | TV UI |
| --- | --- | --- | --- | --- |
| `full` | captures, processes, drives WLED, serves the portal on 8787 | `prysm-webos-service` | `prysm-webos` | `frontend/dist/webos/tv-ui` |
| `slim` | captures and forwards frames to a headless Prysm | `prysm-webos-slim-service` | `prysm-webos-slim` | `frontend/dist/webos-slim/tv-ui` |

Requirements: the relocated webOS SDK (buildroot-nc4,
`arm-webos-linux-gnueabi_sdk-buildroot`), `rustup target add
armv7-unknown-linux-gnueabi`, `ares-package` from `@webos-tools/cli`, Python 3.9+
and Node.js 24+ for the frontend.

```sh
npm --prefix v1/frontend ci && npm --prefix v1/frontend run build   # or pass --frontend
export WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot
v1/packaging/webos/build-ipk.sh full    # v1/target/webos-ipk/full/io.prysm.tv_1.0.0_all.ipk
v1/packaging/webos/build-ipk.sh slim    # v1/target/webos-ipk/slim/io.prysm.tv_1.0.0_all.ipk
```

`--out DIR` changes the output folder; `CARGO_TARGET_DIR` is honoured. The
script builds the flavour's two binaries with `--profile webos` (fat LTO, one
codegen unit, stripped) for Cortex-A9 with `-C target-feature=-soft-float,+neon`
(hardware floating point and NEON while keeping the soft-float calling
convention of the TV's libraries; without it Rust emulates every float
operation and the full worker managed 3 fps at 640x360 with a core pegged,
against 30 fps at about 42% of one core with it; rustc warns that these target
features are unstable, which is expected), then refuses binaries that are not
ARM EABI5 soft-float, are unstripped, need a library outside the TV's base
set or a symbol newer than glibc 2.12 or `CXXABI_1.3.9` ([`ipk.py elf`](webos/ipk.py)). It
packages with `ares-package` and then rewrites the archive: root ownership,
0755 for directories, the two binaries and `setup.sh`, 0644 for everything
else; no install scripts, links, absolute or `..` paths, files outside the
app, service and package directories, or unknown service files. The version
is the workspace version without its pre-release (`app/appinfo.json` must
match). Despite `_all`, the binaries are ARMv7.

The slim worker encodes its stream with H.264 by default
([`prysm-h264`](../adapters/h264/src/lib.rs)): the TV's hardware encoder
(`rvenc`, GStreamer with DMA-BUF input, in
[`h264-rvenc-sys`](../adapters/h264-rvenc-sys/src/lib.rs)) or OpenH264 compiled
in, chosen by the TV preference `capture.encoding` (`h264_hardware`, the
default, `h264_software` or `raw`). It therefore links the sysroot's
GStreamer 1.0 and GLib, which [`env.sh`](webos/env.sh) finds with the SDK's
pkg-config; `ipk.py elf --gstreamer` allows those libraries for the slim
worker only, and the slim package adds the OpenH264, gstreamer-rs and gtk-rs
notices to `licenses/`. OpenH264's C++ needs `CXXABI_1.3.9`, as the old
`io.prysm.capture` worker did on the same TVs. The full package has no encoder.
Desktop builds compile the hardware encoder as a stub and need no GStreamer.

Cross-building by hand: `. v1/packaging/webos/env.sh` in bash (needs
`WEBOS_SDK`) sets the [linker wrapper](webos/linker.sh), the Rust flags, the
C/C++ cross tools and the target pkg-config, then
`cargo build --manifest-path v1/Cargo.toml --target armv7-unknown-linux-gnueabi --profile webos -p prysm-webos-slim`.

Installed layout (`/media/developer/apps/usr/palm/…`):

| Path | Contents |
| --- | --- |
| `applications/io.prysm.tv/` | TV UI bundle, [`appinfo.json`](webos/app/appinfo.json), icon |
| `services/io.prysm.tv.service/prysm-webos-service` | the flavour's supervisor (Luna-activated, root) |
| `services/io.prysm.tv.service/prysm-webos-worker` | the flavour's worker |
| `services/io.prysm.tv.service/setup.sh` | run once from the TV app via Homebrew Channel: `prysm-webos-service setup` |
| `services/io.prysm.tv.service/*.json`, `supervisor.service` | [Luna files](webos/service/): supervisor role (name `io.prysm.tv.service`, inbound any, outbound `tvpower`, `videooutput`), worker role (capture and resource-manager clients), public API `getState setPreferences setEnabled uninstall`, client groups, D-Bus service |
| `services/io.prysm.tv.service/licenses/` | Prysm, hyperion-webos and TV native API notices; slim also OpenH264, gstreamer-rs and gtk-rs |

`setup` writes `/var/luna-service2/manifests.d/io.prysm.tv.{supervisor,worker}.json`
and the boot hook `/var/lib/webosbrew/init.d/io.prysm.tv`; preferences and
settings live in `/var/lib/io.prysm.tv`. The Luna method `uninstall` (from the
TV app) removes the registration before the IPK is removed.

Checks without a TV:

```sh
python3 -m unittest discover -s v1/packaging/webos/tests    # package files and ipk.py
python3 v1/packaging/webos/ipk.py verify v1/target/webos-ipk/full/io.prysm.tv_1.0.0_all.ipk
WEBOS_SDK=… cargo run --manifest-path v1/Cargo.toml -p prysm-webos-luna-sys \
  --features codegen --bin generate-luna-bindings -- --check   # Luna/GLib bindings vs SDK headers
```

The generator needs libclang; without `--check` it rewrites
`adapters/webos-luna-sys/src/generated/` from the SDK headers.

[`tests/native-smoke.py`](webos/tests/native-smoke.py) is the on-TV smoke test
(`--host root@TV --flavour full|slim [--supervisor | --capture]`, `--dry-run`
prints its files and touches nothing). It runs the cargo binaries from the
build directory under a probe identity (`io.prysm.tv.probe`, `run --sandbox`
in a private `/tmp/prysm-supervisor.*`, bounded `--run-for`) and removes its
probe manifest only if unchanged. It has not been run on a TV yet.

Installing on the development TV (webOS 10.3): `ares-install` fails with
"Unable to exec" even after a reboot. Copy the IPK to
`/media/developer/temp/` and install it with
`luna-send -i luna://com.webos.appInstallService/dev/install '{"id":"io.prysm.tv","ipkUrl":"/media/developer/temp/<file>.ipk","subscribe":true}'`
(over ssh, keep stdin open, for example `ssh -t`, or luna-send exits before the
reply). After installing an update the service is not running until
`setup.sh` is run again.

### Checking the slim H.264 encoder on a TV

The slim worker binary has a bounded encoder check that needs no capture, no
Luna role and no receiver (run as root, with the Prysm service disabled so the
hardware encoder is free):

```sh
W=/media/developer/apps/usr/palm/services/io.prysm.tv.service/prysm-webos-worker
timeout 60 $W check-encoder --encoding h264_hardware --width 640 --height 280 --fps 30 --frames 90
timeout 60 $W check-encoder --encoding h264_software --width 640 --height 280 --fps 30 --frames 90
```

It encodes changing synthetic NV12 pictures with exactly that encoder,
decodes each with OpenH264 and compares it with its input (the first picture,
and the first after a session restart half way, must be keyframes of the
current picture), and prints `check-encoder: ok encoder=… bytes=… mbit_s=…
encode_ms_mean=… encode_ms_max=… cpu_percent=…`, then how long releasing the
encoder took. Exit 0 means every check passed; `check-encoder: FAILED: …`
gives the cause chain. `--receiver HOST:7380` then streams `--frames` more of the same pictures (use `--frames 600` for 20 s)
through the real sender to a desktop running
`cargo run --manifest-path v1/Cargo.toml -p prysm-capture-remote --example receive -- 0.0.0.0:7380 60 --pause-at 10`,
which prints the decoded frames per second and pauses and resumes the TV.
