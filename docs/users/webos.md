# Capture from a rooted LG TV

The TV sender replaces an external capture card. You still need Prysm on a
Linux or macOS host, and WLED for physical lighting, with optional browser preview.

**Compatibility:** experimental, tested on LG 43QNED87A6D.BEUFLJP,
webOS 25 (10.3.2-34). Requires a rooted TV and Homebrew Channel root execution.
Other models/firmware are not qualified. Both the TV transport and panel have
no login; keep them on your trusted LAN. Update Prysm and the TV package together.

## 1. Start the receiver

[Install Prysm](README.md). Save the [webOS configuration](examples/webos-wled.toml)
as `prysm-local.toml`, then set the WLED URL and LED counts for your installation.
It listens for the TV on TCP 19400 at 640×360/30 Hz.

Run `prysm --config prysm-local.toml`. Allow TCP port 19400 from your TV through
the host firewall. Note the host's LAN IP; the TV must connect to that address,
not `localhost` or `0.0.0.0`. Dimensions and frame rate are controlled by Prysm.
Open **Preview & tuning** in the browser to inspect the picture alongside WLED output.

## 2. Install the TV app

Download **prysm-webos-armv7** from a successful push or manual
[Check and package run](https://github.com/ericschaal/prysm/actions/workflows/check.yml).
Unzip it; artifacts require GitHub sign-in and expire after three days.
The standard package includes software H.264; hardware H.264 needs an
[explicit custom build](../developers/packaging.md#webos).

Install Node.js 24/npm, then install the [webOS CLI](https://github.com/webos-tools/cli):

```sh
npm install --global @webos-tools/cli@3.2.6
ares-setup-device
```

Add your rooted TV's SSH connection as a device named `TV`. With that connection
configured, run from the download folder:

```sh
ares-install --device TV ./io.prysm.capture_*_all.ipk
```

Keep only the IPK you intend to install in that folder. If an artifact is unavailable,
[build the IPK](../developers/packaging.md#webos). Stop PicCap or any manually
launched capturer before enabling Prysm.

## 3. Connect the sender

Open **Prysm Capture** on the TV:

1. Choose **Set up service** if prompted. Repairs are under
   **Service options → Set up or repair service**.
2. Open **Prysm receiver**, enter the host's LAN IP and port `19400`, and choose
   **Use this address**.
3. Leave **Combined** and **LZ4** selected for the first run; choose **Save changes**.
4. Choose **Enable sender** (or **Save and enable**).

Arrows move focus; OK opens or selects a setting. Save applies changes. Closing
the app leaves the sender enabled. **Enable with TV** optionally enables boot
startup; leave it off until you have checked standby/wake and cold boot on your TV.

## 4. Start capture

Wake the TV and show active video. In Prysm's host panel at `http://localhost:8787`,
select **Screen sync** and turn the lights on .
**Success:** the TV reports **Capturing** and Prysm displays frames or updates the LEDs.

**Ready to capture** is normal while Prysm is idle. Off or WLED effects
releases TV capture resources; the enabled sender reconnects and waits for the
next start. **Waiting for video** means the sender is still enabled, but no video
source is active. Select an HDMI input or play video on the TV; Home and the
Prysm app alone do not provide a capturable video stream. **Capturing** alone
does not guarantee fresh video; inspect Prysm's output too.

## Change settings or remove

**Video only** excludes on-screen menus; **Combined** includes them. UI-only capture
is no longer supported. If upgrading from a UI-only configuration, disable the
sender, back up then remove `/var/lib/prysm-capture/config.json` through your root
TV access, reinstall, and configure again; removing that file resets saved settings.

**Prometheus metrics** is optional and defaults off; saving it restarts an active
worker. See [monitoring](../developers/monitoring.md).

Before uninstalling, choose **Service options → Remove service setup… → Remove setup**,
then uninstall the TV app. This stops the sender and removes capture setup;
saved receiver settings remain. See [troubleshooting](troubleshooting.md#tv-sender).
