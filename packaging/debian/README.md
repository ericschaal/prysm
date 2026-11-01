# Debian arm64

One `prysm-led` package installs `prysm` (embedded web UI, REST API, and pipeline)
and the optional standalone `prysm-web` proxy. Run `prysm.service` for the UI and
API on one address. No Node runtime is needed after building.

## Build

Pushes and manual runs of [Check and package](../../.github/workflows/check.yml)
publish `prysm-debian-arm64` under the run's **Artifacts** section. It contains
the ARM64 `.deb`, including both services and the embedded web UI. Downloads expire
after three days; pull requests still build and verify packages without uploading them.

On Debian arm64, install the Rust toolchain, Node 24, and native capture dependencies:

```sh
sudo apt install build-essential pkg-config clang libclang-dev libudev-dev dpkg-dev
cargo install cargo-deb --version 3.8.0 --locked
./scripts/build-deb.sh
```

The script exports/checks the generated API client, builds/tests the frontend and
Rust application, and creates the package under `target/debian/`. Shared package
files live in `packaging/debian/`; Debian metadata belongs to `apps/prysm/Cargo.toml`.

## Remove an older installation

Older packages used `prysm-led.service` or `prysm-control.service`, stored settings
under `/etc/prysm` or `/var/lib/prysm-control`, and could leave processes running
on removal. These versions require a clean reinstall; settings and service
customizations are deliberately discarded. Skip this section for a fresh install.

```sh
# Stop existing units before removing their binaries. Missing units are harmless.
sudo systemctl stop prysm-led.service prysm-control.service prysm.service prysm-web.service
sudo systemctl disable prysm-led.service prysm-control.service prysm.service prysm-web.service
sudo apt purge prysm-led
sudo rm -rf /etc/prysm /var/lib/prysm-control
# Remove overrides from previous installations too.
sudo rm -f /etc/systemd/system/prysm-led.service /etc/systemd/system/prysm-control.service /etc/systemd/system/prysm.service /etc/systemd/system/prysm-web.service
sudo rm -rf /etc/systemd/system/prysm-led.service.d /etc/systemd/system/prysm-control.service.d /etc/systemd/system/prysm.service.d /etc/systemd/system/prysm-web.service.d
sudo systemctl daemon-reload
```

Files owned by the old Debian package, including its binaries, units, and polkit
rule, are removed by `apt purge`. The unused `prysm-control` system account can
remain; the new services do not use it.

## Install and run

```sh
sudo apt install ./prysm-led_0.1.0-1_arm64.deb
sudoedit /etc/prysm/prysm.toml
sudo -u prysm prysm --config /etc/prysm/prysm.toml check
sudo systemctl enable --now prysm
journalctl -u prysm -f
```

Set `[output].url` to WLED's HTTP base URL; DDP uses the same host on UDP port 4048. Configure input and LED counts. Prefer stable `/dev/v4l/by-id/...`
device paths. The web UI and API share `127.0.0.1:8787`. Open
http://localhost:8787 on the host, or configure LAN access below.

Both services are initially disabled and stopped. The engine runs as `prysm`
with video-group access. Installation seeds `/etc/prysm/prysm.toml` from
`/usr/share/prysm/prysm.toml` only if no configuration exists. The directory is
owned by `prysm` with mode `0700`, and the file starts with mode `0600`. Prysm
needs directory write access because UI saves atomically replace the file.
This is application-managed configuration, not a dpkg conffile. Upgrades preserve it.

The optional `prysm-web.service` is only needed to host a standalone UI for a
remote engine. It uses a separate dynamic user and forwards API requests; it does
not read the engine configuration.

## LAN access

Edit `[web]` in `/etc/prysm/prysm.toml`:

```toml
[web]
listen = "0.0.0.0:8787"
allowed_hosts = ["YOUR_PI:8787"]
```

Restart `prysm` and open `http://YOUR_PI:8787`. The same listen address and host
policy apply to the UI, assets, and API. `allowed_hosts = ["*"]` accepts any
nonempty Host. No systemd override is needed. The unauthenticated UI belongs on a
trusted LAN/private VPN.

## Lifecycle

Upgrades restart services that were running and leave stopped services stopped.
Service enablement and local systemd overrides are preserved. A service restart
never restores saved On intent; starting capture remains explicit.

SIGINT/TERM drains the actor pipeline and joins its thread. Normal capture errors
leave the web UI and API running; retry is explicit. systemd restarts a crashed application.
File input requires FFmpeg 9+ on PATH.

Removal stops both services and preserves `/etc/prysm/prysm.toml`. Purging deletes
that file and removes `/etc/prysm` if empty. Stopping Prysm lets WLED resume its
own effect after realtime timeout; it does not issue a lighting Off command.
