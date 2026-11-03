# Install on Raspberry Pi / Debian arm64

Use 64-bit Raspberry Pi OS or Debian arm64, a video source, and a WLED controller
with DDP enabled. For TV capture, install the host package first, then follow the
[webOS guide](webos.md).

## Get the package

Open [Check and package](https://github.com/ericschaal/prysm/actions/workflows/check.yml),
choose a successful push or manual run, and download **prysm-debian-arm64** from
**Artifacts** (GitHub sign-in required). Unzip it into an otherwise empty folder.
Artifacts expire after three days. If unavailable, [build the package](../developers/packaging.md#debian-arm64).

## Install and configure

Run from the download folder containing only the package you intend to install:

```sh
sudo apt install ./prysm-led_*_arm64.deb
sudo systemctl enable --now prysm
```

The service starts with an empty configuration. Open `http://localhost:8787` on
the host and complete Settings: choose a video source, WLED HTTP base URL, and
LED counts. Save, select **Screen sync**, and turn the lights on. A first
successful save creates `/etc/prysm/settings.json`; no template copy is needed.
For capture modes and wiring, see the [quickstart](quickstart.md).

For a headless Pi, make the panel reachable before opening setup. Run
`sudo systemctl edit prysm` and add your actual LAN address:

```ini
[Service]
Environment="PRYSM_LISTEN=0.0.0.0:8787"
Environment="PRYSM_ALLOWED_HOSTS=192.168.1.20:8787,prysm.example.lan:8787"
```

Restart with `sudo systemctl restart prysm` and open `http://192.168.1.20:8787`.
These process options apply every startup and are never browser settings. Use a trusted LAN or private VPN: the panel has no login.

## Storage and service lifecycle

The package creates `/etc/prysm` owned by `prysm` with mode `0700`. Prysm needs
write access to this directory because saves atomically replace `settings.json`.
Only browser-saved settings live there; environment/CLI options stay outside
the file. JSON is the configuration format; TOML files are not loaded or migrated.

The single service runs as `prysm` with video-group access. Capture starts idle
after every restart. Normal capture failures leave the panel running; systemd
restarts a crashed application. SIGINT/TERM stops and joins capture before exit.
File input requires FFmpeg 9+ on PATH.

The service is initially disabled and stopped at installation. Upgrades restart
running services, preserve enablement and local systemd overrides, and preserve
saved settings. Back up `/etc/prysm` before updating. Removal preserves settings;
purging deletes `settings.json` and removes the directory if empty.

```sh
sudo systemctl stop prysm
sudo apt remove prysm-led
# To delete the package's saved settings as well:
sudo apt purge prysm-led
```

Stopping capture releases WLED realtime control so its own effect resumes after
the realtime timeout; it does not send a lighting Off command. Use
`journalctl -u prysm -f` for errors.
