# webOS capturer

Requires a [rooted LG TV](https://www.webosbrew.org/rooting/) with Homebrew Channel root execution and WLED. Remote mode also requires a [Prysm host](README.md#1-install-the-prysm-application).
Experimental; tested on LG 43QNED87A6D.BEUFLJP, webOS 25 (10.3.2-34).
Other models and firmware are unqualified.

## Processing on this TV (experimental)

1. Install the IPK built with `--embedded-prysm`. Open **Prysm Capture**, set up its service if prompted, choose **Processing location → On this TV (experimental)** and save.
2. On a phone or computer connected to the same LAN, open the **Prysm web portal** address shown on the TV. In [browser setup](configuration.md), set the WLED address and LED layout. You can return to this address to change processing settings, view Preview, or calibrate colors.
3. Allow TV capture, play video, select **Screen sync** in the portal and turn the lights on. The TV processes raw frames locally and sends the resulting colors to WLED.

The portal stays available while the TV is awake, including when capture is paused, video is idle, or the TV app is closed. At startup, **Enable with TV** controls whether capture is allowed. The portal starts whenever embedded mode is selected. Switching to **Remote instance** stops the local portal. Its settings are kept for the next time you select **On this TV**. The standard package offers remote capture. [Building the experimental IPK](../developers/packaging.md#webos)

## 1. Prepare Prysm for remote capture

Open Prysm using its LAN URL, such as `http://192.168.1.20:8787`.
See [Debian](debian.md#2-open-the-control-panel) or
[Docker](docker.md#network-and-deployment-options) for the commands.

In [browser setup](configuration.md), choose **webOS capturer**, 640 × 360,
30 fps. Enter WLED and the LED layout, then **Finish setup**.

## 2. Install on the TV

Download **prysm-webos-armv7** from a successful
[Check and package run](https://github.com/ericschaal/prysm/actions/workflows/check.yml).
Use the same revision as the host. GitHub sign-in is required; artifacts expire
after three days. Or [build the IPK](../developers/packaging.md#webos).

With Node.js 24/npm installed:

```sh
npm install --global @webos-tools/cli@3.2.6
ares-setup-device
```

Add your rooted TV's SSH connection as `TV`. Unzip the artifact into an empty
folder, then install from there:

```sh
ares-install --device TV ./io.prysm.capture_*_all.ipk
```

Stop PicCap or any other capturer.

## 3. Connect

Open **Prysm Capture** on the TV:

1. **Set up service**, if prompted.
2. **Prysm instance URL:** enter `http://YOUR_PRYSM_IP:8787`, then **Use this address**.
3. Keep **Combined** and **LZ4**, then **Save changes**.
4. Choose **Enable** or **Save and enable**.

Combined includes on-screen menus; Video only excludes them. Closing the app
leaves capture enabled. Enable **Enable with TV** only after testing standby/wake
and cold boot.

## 4. Start

Play video on the TV. In Prysm's browser panel, select **Screen sync** and turn
the lights on. Check that [Preview](preview.md) moves and the LEDs follow it.

For errors, see [troubleshooting](troubleshooting.md#webos-capturer).
For tint matching, use [Calibrate Colors](calibration.md).

## Remove

Before uninstalling the app, choose **Service options → Remove service setup… →
Remove setup**. Saved connection settings remain.
