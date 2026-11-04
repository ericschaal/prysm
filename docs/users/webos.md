# webOS capturer

Requires a [rooted LG TV](https://www.webosbrew.org/rooting/) with Homebrew Channel root execution and WLED. Remote mode also requires a [Prysm host](README.md#1-install-the-prysm-application).
Experimental; tested on LG 43QNED87A6D.BEUFLJP, webOS 25 (10.3.2-34).
Other models and firmware are unqualified.

## Processing on this TV (experimental)

Install the IPK built with `--embedded-prysm`. Choose **Processing location → On this TV (experimental)**, open **WLED setup**, and enter the controller address and LED counts for the top, right, bottom and left edges. Choose **Save and enable** and play video. The TV captures and processes raw frames locally and sends colors directly to WLED. Closing the app keeps the supervised worker enabled. The first version uses the built-in processing defaults; receiver-based calibration is available in remote mode.

Choose **Remote instance** to switch back and enter or reuse the receiver URL. The TV keeps the WLED setup when switching modes. The standard package offers remote capture. [Building the experimental IPK](../developers/packaging.md#webos)

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
