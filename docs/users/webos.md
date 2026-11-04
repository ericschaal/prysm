# Capture from a rooted LG TV

The **webOS capturer** captures video on your rooted LG TV and
sends it to the Prysm application over your LAN, replacing an HDMI capture card.
You still need a [Prysm host and WLED controller](README.md).

This is experimental. It is tested on LG 43QNED87A6D.BEUFLJP with webOS 25
(10.3.2-34). It requires a rooted TV and Homebrew Channel root execution;
other models and firmware are not qualified. Keep the TV and Prysm on a trusted
LAN: neither the panel nor the TV connection has a login.

## 1. Prepare the Prysm host

[Install and start Prysm](README.md#1-install-the-prysm-application). Make its
panel reachable from the TV and your phone, using the host's LAN address such as
`http://192.168.1.20:8787`. Follow your installation guide for LAN access.

In the [browser setup wizard](configuration.md), choose **webOS capturer**.
Start at 640 × 360 and 30 fps, enter WLED and your LED layout, then finish setup.
webOS capture requires even dimensions, up to 1920 × 1080 and 60 fps; your TV
may support less. Leave screen sync off until the webOS capturer is ready.

## 2. Install the TV app

There is no official release yet. Download **prysm-webos-armv7** from a successful
push or manual [Check and package run](https://github.com/ericschaal/prysm/actions/workflows/check.yml).
Use the same code revision for the TV package and Prysm host. Artifacts require
GitHub sign-in and expire after three days. If none is available,
[build the TV package](../developers/packaging.md#webos).

Install Node.js 24/npm and the [webOS CLI](https://github.com/webos-tools/cli):

```sh
npm install --global @webos-tools/cli@3.2.6
ares-setup-device
```

Add your rooted TV's SSH connection as a device named `TV`. Unzip the artifact
into an empty folder, then install its IPK from that folder:

```sh
ares-install --device TV ./io.prysm.capture_*_all.ipk
```

Stop PicCap or any other capturer before enabling Prysm.

## 3. Connect the webOS capturer

Open **Prysm Capture** on the TV:

1. Choose **Set up service** if prompted.
2. Open **Prysm instance URL**, enter the host panel address such as
   `http://192.168.1.20:8787`, and choose **Use this address**.
3. Keep **Combined** and **LZ4** selected. Choose **Save changes**.
4. Choose **Enable**, or **Save and enable**.

**Combined** includes on-screen menus; **Video only** excludes them. Saving
checks the connection to Prysm. Closing the TV app leaves the webOS capturer enabled.
Leave **Enable with TV** off until you have checked standby, wake, and cold boot
on your TV.

## 4. Start Screen sync

Show active video on the awake TV. In the Prysm host's browser panel, select
**Screen sync** and turn the lights on. Success means the TV reports
**Capturing**, Prysm's [Preview](preview.md) shows advancing frames, and the
LEDs follow the picture.

**Ready to capture** means the TV has video and Prysm is idle.
**Waiting for video** means the webOS capturer is enabled but the TV has no active video.
Play a video or select an active HDMI input. Choosing Off or WLED effects in
Prysm releases the TV's capture resources.

## Change settings or remove

Change the TV connection and capture mode in the TV app. Change WLED, LED layout,
and picture processing in the host's browser panel. Use
[Calibrate Colors](calibration.md) after screen sync works.

Repairs are under **Service options → Set up or repair service**.
Before uninstalling the TV app, choose **Service options → Remove service
setup… → Remove setup**. This stops the webOS capturer and removes service setup;
its saved connection settings remain.

For connection errors, see [TV troubleshooting](troubleshooting.md#webos-capturer).
For HTTPS, proxy the whole Prysm site, including WebSocket connections, and
allow the external hostname in `PRYSM_ALLOWED_HOSTS`. The TV uses the same panel
address for capture and calibration; there is no separate capture port.
