# Packaging prysm-headless

Two ways to run the [headless host](../crates/hosts/headless/src/main.rs) on Linux:
a Docker image and a Debian package with a systemd service. Both are built by
[`docker/Dockerfile`](docker/Dockerfile) from the repository root, for
`linux/amd64` and `linux/arm64` (Raspberry Pi OS 64-bit). They assume a machine
without the earlier implementation's image or `prysm-led` package.

Both offer the `device` (V4L2) and `remote` sources and the `wled` and `null`
outputs. The `file` source (play a video file) is for development only: it
needs FFmpeg 9 or newer on `PATH`, and neither the Docker image nor the Debian
package includes FFmpeg, so `file` does not work in either. To try it, run
`prysm-tui` or `prysm-headless` from a source checkout on a machine with
FFmpeg 9+ installed ([hosts](../docs/developers/workspace.md#hosts)).

## Portal and settings

The portal listens on port **8787** on every interface: open
`http://<machine>:8787`. It has no password, so anyone who can reach the port
can control the lights; keep it on a trusted LAN. Browser pages from other
sites are refused unless allowed with `--allow-origin`
([access](../crates/surfaces/web-portal/HTTP-CONTRACT.md#access)).

Settings are one JSON file ([format](../crates/adapters/settings-file/README.md),
[example](../crates/hosts/headless/settings.example.json)). A missing file means
"unconfigured": the host starts idle and the portal writes the file on the
first save. Hosts start idle; add `--start` to request lighting at launch.

| | Docker | Debian |
| --- | --- | --- |
| Settings | `/data/settings.json` (volume) | `/var/lib/prysm/settings.json` |
| Runs as | UID/GID 10001, plus `video` (GID 44) | systemd dynamic user `prysm-headless`, plus `video` |
| Logs | `docker logs` | `journalctl -u prysm-headless` |
| Extra options | arguments after the image name | `systemctl edit prysm-headless` |

`RUST_LOG` filters log lines (default `warn,prysm=info`).

## Docker

Build (BuildKit/buildx required; `--platform linux/arm64` for a Pi):

```sh
docker buildx build -f packaging/docker/Dockerfile -t prysm-headless:local --load .
packaging/docker/smoke.sh prysm-headless:local   # optional check
```

Run with a USB capture device:

```sh
docker run -d --name prysm --restart unless-stopped --stop-timeout 30 \
  -p 8787:8787 -v prysm-data:/data \
  --device /dev/video0 --group-add "$(getent group video | cut -d: -f3)" \
  prysm-headless:local
```

or `docker compose -f packaging/docker/compose.yaml up -d`
([compose.yaml](docker/compose.yaml)). Notes:

- Arguments after the image name are appended to
  `prysm-headless /data/settings.json`, e.g. `--start` or
  `--allow-origin http://192.168.1.30:3000`.
- A bind mount instead of the `prysm-data` volume must be writable by UID
  10001 (`chown 10001:10001 DIR`).
- A slim TV (`remote` source) streams to the portal's port; nothing else to
  publish.
- A bridge network cannot resolve mDNS names such as `wled.local`; use the
  WLED's IP address or `--network host`.
- The image's `HEALTHCHECK` reads `GET /api/v1/state` on port 8787; it reports
  unhealthy with `--no-portal` or another `--listen` port.
- `docker stop` sends SIGTERM; the host idles the LEDs and exits. Allow 30 s.
- The image has no FFmpeg, so the development-only `file` source fails to
  start; use `device` or `remote`.

Release images are published as `ghcr.io/ericschaal/prysm-headless:<version>`
(and `:latest` for the newest stable release) by
[release.yml](../.github/workflows/release.yml); see
[releasing](../docs/developers/releasing.md).

## Debian package

Build the package for the build machine's architecture (any OS with Docker;
`deb-arm64` for a Pi):

```sh
cargo xtask dist deb-arm64   # dist/prysm-headless-linux-arm64.deb
```

That is the Dockerfile's `artifacts` target, which also holds the binary:
`docker buildx build -f packaging/docker/Dockerfile --platform linux/arm64
--target artifacts --output type=local,dest=DIR .` writes
`prysm-headless-linux-arm64` and `prysm-headless-linux-arm64.deb`. Releases
attach the package as `prysm-headless_<version>_<arch>.deb`.
[`debian/build-deb.sh BINARY OUT`](debian/build-deb.sh) does the
assembly with `dpkg-deb` and can run on any Debian/Ubuntu with `dpkg-dev`.
The binary is built on Debian 12 and needs systemd and glibc 2.34 or newer
(Debian 12+, Ubuntu 22.04+, Raspberry Pi OS bookworm, 64-bit).

Install and use:

```sh
sudo apt install ./prysm-headless*.deb    # enables and starts the service
systemctl status prysm-headless
journalctl -u prysm-headless -f
sudo systemctl edit prysm-headless        # e.g. ExecStart= then ExecStart=/usr/bin/prysm-headless /var/lib/prysm/settings.json --start
```

The package installs `/usr/bin/prysm-headless` and the
[unit](debian/prysm-headless.service), and creates no account. The unit runs
as a systemd dynamic user (`DynamicUser=yes`) with `video` for `/dev/video*`,
keeps its state in `StateDirectory=prysm` (`/var/lib/private/prysm`, linked
from `/var/lib/prysm`), restarts on failure and is sandboxed (read-only
system, no capabilities, only V4L2 devices, IPv4/IPv6/Unix sockets).
`apt remove` stops and disables the service; `apt purge` also deletes the
state directory and its link. The package conflicts with the earlier implementation's `prysm-led`
(both would take the capture device and the portal port), and ships the
OpenH264 notice next to its copyright file, as the image does in
`/usr/share/doc/prysm-headless/`.

The package does not depend on FFmpeg, so the development-only `file` source
does not work with it; use `device` or `remote`.

## webOS TV packages

Two IPKs for a rooted LG TV ([how they run](../docs/developers/webos.md)),
built by [`webos/build-ipk.sh`](webos/build-ipk.sh). Both are the app
`io.github.ericschaal.prysm` with the service `io.github.ericschaal.prysm.service` and the same installed
file names, so installing one replaces the other. Nothing detects or migrates
the old `io.prysm.capture` app: remove it (and its service setup) first.

| Flavour | The TV… | Supervisor (cargo bin) | Worker (cargo bin) | TV UI |
| --- | --- | --- | --- | --- |
| `full` | captures, processes, drives WLED, serves the portal on 8787 | `prysm-webos-service` | `prysm-webos` | `frontend/dist/webos/tv-ui` |
| `slim` | captures and forwards frames to a headless Prysm | `prysm-webos-slim-service` | `prysm-webos-slim` | `frontend/dist/webos-slim/tv-ui` |

Picture settings (size, rate, menus, encoding) live in a web portal in both
flavours, never in the TV's Luna preferences (`tv.json` holds only
`autostart`, and for slim `instance`). Full: the `webos`
capture section of the worker's `settings.json`, edited in the TV's own
portal (default 640x360 at 30 fps). Slim: the `remote` capture section of the
desktop Prysm (`width`, `height`, `fps`, `include_ui`, `encoding`; default
320x180 at 30 fps with menus, hardware H.264), which the receiver sends to the
TV on every connection ([wire protocol](../crates/adapters/capture-remote/README.md));
the TV captures and encodes as asked. Saving other capture settings restarts
lighting, the TV reconnects within a second and follows the new request.
`instance` is the desktop Prysm's portal address (`http://` or `https://`),
the host an IP address or a name such as `prysm.local` (checked for syntax
only when saved); the slim worker resolves it at every connection attempt,
waiting at most 0.5 s per attempt (a slower answer is used by the next one),
so an address change is picked up on reconnect and a name that does not
resolve shows as waiting for the receiver.
While the desktop's lighting is off its receiver has stopped the stream: the
TV closes its capture and reports `paused` (not a failure) until lighting
starts again. When a second TV sends to the same desktop it takes the stream
over; the first reports a `replaced` failure and stops sending until its
worker restarts (capture stopped and started, the TV waking, or a new Prysm
address).

The slim TV app controls the desktop Prysm's portal from `Origin: null` (a
packaged webOS app). Start the desktop Prysm with `--allow-origin null` for
that, e.g. `docker run … prysm-headless:local --allow-origin null` or
`ExecStart=/usr/bin/prysm-headless /var/lib/prysm/settings.json --allow-origin
null`; without it the TV app says how to allow it. `null` is also the origin
of sandboxed frames on any site, so any web page opened on that network can
then drive the lights, as with the portal's other LAN exposure.

`javascript.txt` comes from the frontend build: for each host,
[`scripts/licenses.mjs`](../frontend/scripts/licenses.mjs) records which
packages under `node_modules` end up in that host's bundles (the TV UI and,
for `full`, the portal the worker serves) and writes their names, versions
and licence files to `frontend/dist/<host>/licenses.txt`, which the build
copies into the package. The build fails if a bundled package has no licence
file.

Requirements: the relocated webOS SDK (buildroot-nc4,
`arm-webos-linux-gnueabi_sdk-buildroot`), `rustup target add
armv7-unknown-linux-gnueabi`, `ares-package` from `@webos-tools/cli`, Python 3.9+
and Node.js 24+ for the frontend.

```sh
export WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot
cargo xtask dist tv-full tv-slim     # dist/prysm-tv-full.ipk, dist/prysm-tv-slim.ipk
# or, after npm --prefix frontend ci && npm --prefix frontend run bundle (or with --frontend):
packaging/webos/build-ipk.sh full    # target/webos-ipk/full/io.github.ericschaal.prysm-full_1.0.0_all.ipk
packaging/webos/build-ipk.sh slim    # target/webos-ipk/slim/io.github.ericschaal.prysm-slim_1.0.0_all.ipk
```

The script prints the package's path on standard output and everything else
on standard error.

`--out DIR` changes the output folder; `CARGO_TARGET_DIR` is honoured. The
script builds the flavour's two binaries with `--profile webos` (fat LTO, one
codegen unit, stripped, integer overflow checked) for Cortex-A9 with `-C target-feature=-soft-float,+neon`
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
app, service and package directories, or unknown service files. Despite
`_all`, the binaries are ARMv7.

The version has one source: [`VERSION`](../VERSION) (see
[releasing](../docs/developers/releasing.md#the-version-is-one-file)). The binaries report it in full
(`prysm-webos-service --version` prints, for example, `1.0.0-rc.1`). The app's
`appinfo.json`, and with it the IPK's file name and control file, carry only
its `major.minor.patch` (`1.0.0`), because `ares-package` accepts nothing but
three numbers there. The `appinfo.json` in this repository has no version;
[`ipk.py appinfo`](webos/ipk.py) adds it while packaging.

The slim worker encodes its stream with H.264 by default
([`prysm-h264`](../crates/adapters/h264/src/lib.rs)): the TV's hardware encoder
(`rvenc`, GStreamer with DMA-BUF input, in
[`h264-rvenc-sys`](../crates/adapters/h264-rvenc-sys/src/lib.rs)) or OpenH264 compiled
in, as the desktop's `remote` capture `encoding` requests (`h264_hardware`,
the default, `h264_software` or `raw`). If that encoder fails the TV reports
`failure.kind` `encoder` and refuses the request, so lighting on the desktop
stops with the TV's reason, until another encoding is saved there; nothing
falls back on its own. It therefore links the sysroot's
GStreamer 1.0 and GLib, which [`env.sh`](webos/env.sh) finds with the SDK's
pkg-config; `ipk.py elf --gstreamer` allows those libraries for the slim
worker only, and the slim package adds the OpenH264, gstreamer-rs and gtk-rs
notices to `licenses/`. OpenH264's C++ needs `CXXABI_1.3.9`, as the old
`io.prysm.capture` worker did on the same TVs. The full package has no encoder.
Desktop builds compile the hardware encoder as a stub and need no GStreamer.

Cross-building by hand: `. packaging/webos/env.sh` in bash (needs
`WEBOS_SDK`) sets the [linker wrapper](webos/linker.sh), the Rust flags, the
C/C++ cross tools and the target pkg-config, then
`cargo build --target armv7-unknown-linux-gnueabi --profile webos -p prysm-webos-slim`.

Installed layout (`/media/developer/apps/usr/palm/…`):

| Path | Contents |
| --- | --- |
| `applications/io.github.ericschaal.prysm/` | TV UI bundle, [`appinfo.json`](webos/app/appinfo.json), icon |
| `services/io.github.ericschaal.prysm.service/prysm-webos-service` | the flavour's supervisor (Luna-activated, root) |
| `services/io.github.ericschaal.prysm.service/prysm-webos-worker` | the flavour's worker |
| `services/io.github.ericschaal.prysm.service/setup.sh` | run after every install or update (TV app via Homebrew Channel, `install-ipk.sh`, or ssh): `prysm-webos-service setup` |
| `services/io.github.ericschaal.prysm.service/*.json`, `supervisor.service` | [Luna files](webos/service/): supervisor role (name `io.github.ericschaal.prysm.service`, inbound any, outbound `tvpower`, `videooutput`), worker role (capture and resource-manager clients), public API `getState setPreferences setTarget setEnabled uninstall`, client groups, D-Bus service |
| `services/io.github.ericschaal.prysm.service/licenses/` | Prysm, hyperion-webos and TV native API notices, `javascript.txt` for the bundled JavaScript; slim also OpenH264, gstreamer-rs and gtk-rs |

`setup` writes `/var/luna-service2/manifests.d/io.github.ericschaal.prysm.{supervisor,worker}.json`
and the boot hook `/var/lib/webosbrew/init.d/io.github.ericschaal.prysm`; preferences and
settings live in `/var/lib/io.github.ericschaal.prysm` (`tv.json`, `resume.json`, full:
`settings.json`). The Luna method `uninstall` (from the
TV app) removes the registration before the IPK is removed.

Checks without a TV:

```sh
python3 -m unittest discover -s packaging/webos/tests    # package files and ipk.py
python3 packaging/webos/ipk.py verify target/webos-ipk/full/io.github.ericschaal.prysm-full_1.0.0_all.ipk
WEBOS_SDK=… cargo run -p prysm-webos-luna-sys \
  --features codegen --bin generate-luna-bindings -- --check   # Luna/GLib bindings vs SDK headers
```

The generator needs libclang; without `--check` it rewrites
`crates/adapters/webos-luna-sys/src/generated/` from the SDK headers.

[`tests/native-smoke.py`](webos/tests/native-smoke.py) is the on-TV smoke test
(`--host root@TV --flavour full|slim [--supervisor | --capture]`, `--dry-run`
prints its files and touches nothing). It runs the cargo binaries from the
build directory under a probe identity (`io.github.ericschaal.prysm.probe`, `run --sandbox`
in a private `/tmp/prysm-supervisor.*`, bounded `--run-for`) and removes its
probe manifest only if unchanged. No run on a TV is recorded yet.

### Installing on a TV

`ares-install` fails with "Unable to exec" on webOS 10 (even after a reboot).
Install through the TV's install service instead;
[`webos/install-ipk.sh`](webos/install-ipk.sh) does every step
(`--dry-run` prints them):

```sh
packaging/webos/install-ipk.sh root@TV target/webos-ipk/full/io.github.ericschaal.prysm-full_1.0.0_all.ipk
```

The helper first stops a running Prysm through its Luna `uninstall` and
installs nothing while a process under `io.github.ericschaal.prysm.*` is left: the install
service would end it abruptly
([webOS](../docs/developers/webos.md#files-on-the-tv)).

By hand (wait until no Prysm process is left before the install):

```sh
ssh root@TV 'mkdir -p /media/developer/temp && cat > /media/developer/temp/io.github.ericschaal.prysm.ipk' < io.github.ericschaal.prysm-full_1.0.0_all.ipk
(sleep 14) | ssh root@TV "timeout 12 luna-send -a io.github.ericschaal.prysm -n 1 luna://io.github.ericschaal.prysm.service/uninstall '{}'"
(sleep 122) | ssh root@TV "timeout 120 luna-send -i luna://com.webos.appInstallService/dev/install \
  '{\"id\":\"io.github.ericschaal.prysm\",\"ipkUrl\":\"/media/developer/temp/io.github.ericschaal.prysm.ipk\",\"subscribe\":true}'"
ssh root@TV /media/developer/apps/usr/palm/services/io.github.ericschaal.prysm.service/setup.sh
```

luna-send waits for replies only while its stdin is open, so hold it open with
`(sleep N) | ssh …` and bound it with `timeout` (`ssh -t` also works
interactively). Wait for the `installed` reply before the next step.

**After an install or update, run `setup.sh` once** (the helper does; the TV
app's set-up action and the boot hook run the same `prysm-webos-service
setup`): the service is not running again until then, and nothing in the
package can trigger it, since IPKs carry no install scripts. Setup rescans
and activates the service, which then resumes what it was doing: the
supervisor keeps the intent (full: lighting target; slim: enabled) with the
boot id in `/var/lib/io.github.ericschaal.prysm/resume.json`, so a restart in the same boot
restores it, and a reboot starts from `autostart` as before.

### Checking the slim H.264 encoder on a TV

The slim worker binary has a bounded encoder check that needs no capture, no
Luna role and no receiver (run as root, with the Prysm service disabled so the
hardware encoder is free):

```sh
W=/media/developer/apps/usr/palm/services/io.github.ericschaal.prysm.service/prysm-webos-worker
timeout 60 $W check-encoder --encoding h264_hardware --width 640 --height 280 --fps 30 --frames 90
timeout 60 $W check-encoder --encoding h264_software --width 640 --height 280 --fps 30 --frames 90
```

It encodes changing synthetic NV12 pictures with exactly that encoder,
decodes each with OpenH264 and checks that every 16x16 block shows its own
picture, not one up to two pictures before or after (the first picture, and
the first after a session restart half way, must be keyframes of the current
picture), and prints `check-encoder: ok encoder=… bytes=… mbit_s=…
encode_ms_mean=… encode_ms_max=… cpu_percent=…`, then how long releasing the
encoder took. Exit 0 means every check passed; `check-encoder: FAILED: …`
gives the cause chain. `--prysm http://HOST:8787` (or an `https://`
address) then streams `--frames` more of the same pictures (use `--frames
600` for 20 s) through the real sender, at the default bitrate, to a desktop
Prysm whose `remote` capture requests that stream (for example 640 × 280,
Hardware H.264, with the `null` output) and whose lights are on; its portal
shows the received rate. The check refuses a Prysm whose request differs
from its own flags.
