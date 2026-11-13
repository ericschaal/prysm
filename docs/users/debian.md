# Raspberry Pi and Debian

The `prysm-headless` package runs Prysm as a systemd service. It needs a
64-bit system with systemd and glibc 2.34 or newer: Raspberry Pi OS (bookworm,
64-bit), Debian 12 or newer, or Ubuntu 22.04 or newer, on arm64 or amd64.

*Not yet verified on hardware:* the package is built and install-tested in CI,
but not yet with a capture card and WLED on a Raspberry Pi.

## 1. Install

Download `prysm-headless_*_arm64.deb` (Raspberry Pi) or `..._amd64.deb` from a
`v1.*` [release](https://github.com/ericschaal/prysm/releases), or
[build it](../developers/packaging.md#debian-package). In the download
directory, run:

```sh
sudo apt install ./prysm-headless_*.deb
```

This creates the system user `prysm`, then enables and starts the
`prysm-headless` service.

## 2. Open the portal

On the Pi, open `http://localhost:8787`. From a phone or another computer,
open `http://PI_IP:8787`, for example `http://192.168.1.20:8787`. Keep it on a
trusted network; the portal has no password.

## 3. Connect video and lights

Connect a [capture card](quickstart.md) or the [slim TV package](webos.md),
then [set up your lights](configuration.md).

## Manage the service

```sh
systemctl status prysm-headless
sudo systemctl restart prysm-headless
journalctl -u prysm-headless -f
```

Settings live in `/var/lib/prysm/settings.json`. Back it up before upgrades.
Restarting leaves the lights off; stopping the service turns them off.

To add options, such as starting Screen sync at boot (`--start`) or allowing
the slim TV app (`--allow-origin null`), run `sudo systemctl edit prysm-headless`
and add:

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/prysm-headless /var/lib/prysm/settings.json --start
```

Then run `sudo systemctl restart prysm-headless`. See all options with
`prysm-headless --help`.

The service can open video devices (`/dev/video*`) and the network, nothing
else. The slim TV sends its picture to the portal's port (8787), so that is
the only port to allow in a firewall.

## Remove

`sudo apt remove prysm-headless` stops and disables the service and keeps the
settings. `sudo apt purge prysm-headless` also deletes `/var/lib/prysm`. The
`prysm` user stays.
