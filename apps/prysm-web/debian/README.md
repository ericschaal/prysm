# Debian arm64

One package installs `prysm` (REST API and pipeline) and `prysm-web` (embedded
React UI and API forwarding), with separate `prysm-control` and `prysm-web`
systemd units. The engine has no dependency on the browser controller. No Node runtime is needed after building.

## Build

On Debian arm64, install the Rust toolchain, Node 24, and native capture dependencies:

```sh
sudo apt install build-essential pkg-config clang libclang-dev libudev-dev dpkg-dev
cargo install cargo-deb --version 3.8.0 --locked
./scripts/build-deb.sh
```

The script exports/checks the generated API client, builds/tests the frontend and
Rust application, and creates the package under `target/debian/`.

## Install and run

```sh
sudo apt install ./prysm-led_0.1.0-1_arm64.deb
sudoedit /var/lib/prysm-control/prysm.toml
sudo prysm --config /var/lib/prysm-control/prysm.toml check
sudo systemctl enable --now prysm-control prysm-web
journalctl -u prysm-control -f
```

Set `[wled].url` to WLED's HTTP endpoint and `[output].address` to its DDP address
(port 4048). Configure input, LED counts, web listen address, and allowed hosts.
Prefer stable `/dev/v4l/by-id/...` device paths. Open http://YOUR_PI:8788. The REST API stays on port 8787.

The service starts disabled. It runs as `prysm-control` with video-group access.
Installation seeds `/var/lib/prysm-control/prysm.toml` from the packaged template
at `/usr/share/prysm/prysm.toml`, only if no runtime file exists. The state folder
is private to the service user; Prysm atomically saves all UI settings
into this single writable file. Upgrades preserve it.

SIGINT/TERM drains the actor pipeline and joins its thread. Normal capture errors
leave the web UI running; retry is explicit. systemd restarts a crashed application,
but startup never restores saved On intent. File input requires FFmpeg 9+ on PATH.

Removal stops both services and preserves its state directory. Delete that
folder explicitly to discard saved settings. Stopping Prysm lets WLED resume its
own effect after realtime timeout; it does not issue a lighting Off command.
The unauthenticated panel belongs on a trusted LAN/private VPN.
