# Install on Raspberry Pi / Debian arm64

Use a Raspberry Pi with 64-bit Raspberry Pi OS, or another Debian arm64 host.
This installs the Prysm application and its browser panel as a service. Your
[capture device and WLED controller](README.md) are configured afterward.

## 1. Get and install the package

There is no official release yet. Open [Check and package](https://github.com/ericschaal/prysm/actions/workflows/check.yml),
choose a successful push or manual run, and download **prysm-debian-arm64** from
**Artifacts**. GitHub sign-in is required; artifacts expire after three days.
If none is available, [build the package](../developers/packaging.md#debian-arm64).

Unzip the artifact into an empty folder. From that folder, run:

```sh
sudo apt install ./prysm-led_*_arm64.deb
sudo systemctl enable --now prysm
```

The service runs as user `prysm`, with access to the `video` group.

## 2. Open the control panel

On the Prysm host, open `http://localhost:8787`.

For a headless Pi or access from your phone, run `sudo systemctl edit prysm`
and add this, replacing `192.168.1.20` with the Pi's LAN address:

```ini
[Service]
Environment="PRYSM_ALLOWED_HOSTS=localhost:8787,127.0.0.1:8787,192.168.1.20:8787"
```

Then run:

```sh
sudo systemctl restart prysm
```

Open `http://192.168.1.20:8787` from a device on the same trusted LAN. The panel
has no login.

## 3. Connect video and lights

Follow [capture card setup](quickstart.md) or [webOS capturer setup](webos.md),
then [complete the browser wizard](configuration.md). Success means the LEDs
follow the picture when **Screen sync** is on.

Settings are saved in `/etc/prysm/settings.json`. The service must be able to
write the whole `/etc/prysm` directory. Back it up before changing the installation.
Prysm starts idle after every restart.

## Service commands

```sh
sudo systemctl status prysm
sudo journalctl -u prysm -f
sudo systemctl stop prysm
sudo systemctl restart prysm
```

Stopping the service releases live color control; WLED can resume its own effect
after its realtime timeout. To turn the lights off, use Prysm's power control
before stopping the service.

`sudo apt remove prysm-led` preserves settings. `sudo apt purge prysm-led`
deletes the saved settings.
