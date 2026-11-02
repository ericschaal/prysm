# Install on Raspberry Pi / Debian arm64

Use 64-bit Raspberry Pi OS or Debian arm64, a capture device, and a working
WLED controller with DDP enabled. For TV capture, use the [webOS guide](webos.md)
after installing the host package.

## 1. Get the package

Open [Check and package](https://github.com/ericschaal/prysm/actions/workflows/check.yml),
choose a successful push or manual run, and download **prysm-debian-arm64** from
**Artifacts** (GitHub sign-in required). Unzip it into an otherwise empty folder.
Artifacts expire after three days. If none are available, [build the package](../developers/packaging.md#debian-arm64).

## 2. Install and configure

Run these commands from the download folder, containing only the package you intend to install.
For old multi-service installations, read [legacy installations](#legacy-installations) first.

```sh
sudo apt install ./prysm-led_*_arm64.deb
sudoedit /etc/prysm/prysm.toml
sudo -u prysm prysm --config /etc/prysm/prysm.toml check
sudo systemctl enable --now prysm
```

Set `[output].url` to WLED's HTTP base URL; DDP uses the same host on UDP port 4048. Configure input and LED counts. Prefer stable `/dev/v4l/by-id/...`
device paths. The web UI and API share `127.0.0.1:8787`. Open
http://localhost:8787 on the host, or use the LAN access instructions below.

## 3. Start the lights

Open `http://localhost:8787` on the host, select **Screen sync**, and turn the lights on.
**Success:** the panel reports active screen sync and the LEDs follow the picture.
The service starts Prysm automatically at boot, but capture always starts idle.

For your phone, [enable LAN access](configuration.md#access-from-your-phone).
For capture setup and LED orientation, see the [quickstart](quickstart.md).

## Upgrade, stop, and remove

The service is initially disabled and stopped. The engine runs as `prysm`
with video-group access. Installation seeds `/etc/prysm/prysm.toml` from
`/usr/share/prysm/prysm.toml` only if no configuration exists. The directory is
owned by `prysm` with mode `0700`, and the file starts with mode `0600`. Prysm
needs directory write access because UI saves atomically replace the file.
This is application-managed configuration, not a dpkg conffile. Upgrades preserve it.

Upgrading from the package with the optional standalone proxy stops and disables
`prysm-web.service` before removing its binary and packaged unit. Prysm settings
and the enablement of `prysm.service` are preserved. Remote browsers now connect
directly to Prysm's configured web address.

Upgrades restart services that were running and leave stopped services stopped.
Service enablement and local systemd overrides are preserved. A service restart
never restores saved On intent; starting capture remains explicit.

SIGINT/TERM drains the actor pipeline and joins its thread. Normal capture errors
leave the web UI and API running; retry is explicit. systemd restarts a crashed application.
File input requires FFmpeg 9+ on PATH.

Removal stops the service and preserves `/etc/prysm/prysm.toml`. Purging deletes
that file and removes `/etc/prysm` if empty. Stopping Prysm lets WLED resume its
own effect after realtime timeout; it does not issue a lighting Off command.

## Legacy installations

Back up settings and local systemd overrides first. This procedure deletes settings
and service customizations from older multi-service packages. Current single-service
packages upgrade in place and do not need this cleanup.

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
remain; the new service does not use it.
