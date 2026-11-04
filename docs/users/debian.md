# Raspberry Pi / Debian arm64

Requires 64-bit Raspberry Pi OS or Debian arm64.

## 1. Install

Download **prysm-debian-arm64** from a successful
[Check and package run](https://github.com/ericschaal/prysm/actions/workflows/check.yml).
GitHub sign-in is required; artifacts expire after three days.
Alternatively, [build the package](../developers/packaging.md#debian-arm64).

Unzip into an empty folder, then run there:

```sh
sudo apt install ./prysm-led_*_arm64.deb
sudo systemctl enable --now prysm
```

## 2. Open the control panel

On the Pi, open `http://localhost:8787`.

For a phone or another computer, run `sudo systemctl edit prysm` and add this,
replacing `192.168.1.20` with the Pi's LAN IP:

```ini
[Service]
Environment="PRYSM_ALLOWED_HOSTS=localhost:8787,127.0.0.1:8787,192.168.1.20:8787"
```

Run `sudo systemctl restart prysm`, then open `http://192.168.1.20:8787`.
Use a trusted LAN; Prysm has no login.

## 3. Connect video and lights

Connect a [capture card](quickstart.md) or [webOS capturer](webos.md), then
[set up WLED and Screen sync](configuration.md).

## Manage the service

```sh
sudo systemctl stop prysm
sudo systemctl restart prysm
sudo journalctl -u prysm -f
```

Settings: `/etc/prysm/settings.json`. Back up `/etc/prysm` and keep it writable
by user `prysm`. Restarting Prysm leaves Screen sync off.

Stopping the service can return the lights to a WLED effect. Turn them off in
the panel first if you want them off.

`sudo apt remove prysm-led` keeps settings; `sudo apt purge prysm-led` deletes them.
