# webOS capturer

Requires a [rooted LG TV](https://www.webosbrew.org/rooting/) with Homebrew Channel root execution, a
[Prysm host](README.md#1-install-the-prysm-application), and WLED.
Experimental; tested on LG 43QNED87A6D.BEUFLJP, webOS 25 (10.3.2-34).
Other models and firmware are unqualified.

## 1. Prepare Prysm

Open Prysm using its LAN URL, such as `http://192.168.1.20:8787`.
See [Debian](debian.md#2-open-the-control-panel) or
[Docker](docker.md#network-and-deployment-options) for the commands.

In [browser setup](configuration.md), choose **webOS capturer**, 640 × 360,
30 fps. Enter WLED and the LED layout, then **Finish setup**.

## 2. Install on the TV

Download `io.prysm.capture_*_all.ipk` from the
[same release as the host](https://github.com/ericschaal/prysm/releases).
The IPK includes hardware H.264 support. Or [build it](../developers/packaging.md#webos).

With Node.js 24/npm installed:

```sh
npm install --global @webos-tools/cli@3.2.6
ares-setup-device
```

Add your rooted TV's SSH connection as `TV`, then install from the download directory:

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
