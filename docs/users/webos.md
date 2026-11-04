# Capture from a rooted LG TV

The TV sender replaces an external capture card. You still need Prysm on a
Linux or macOS host, and WLED for physical lighting, with optional browser preview.

**Compatibility:** experimental, tested on LG 43QNED87A6D.BEUFLJP,
webOS 25 (10.3.2-34). Requires a rooted TV and Homebrew Channel root execution.
Other models/firmware are not qualified. Both the TV transport and panel have
no login; keep them on your trusted LAN. Update Prysm and the TV package together.

## 1. Start the receiver

[Install Prysm](README.md), then open the panel and complete Settings. Choose
webOS input, capture resolution 640 × 360 and
30 Hz. Enter the WLED URL and LED counts and save. No initial configuration file
is needed; the first save creates the settings JSON.

The TV and phone must both be able to open the Prysm panel address. For direct LAN access, publish the HTTP listener on the trusted network and add its authority (for example `192.168.1.10:8787`) to `PRYSM_ALLOWED_HOSTS`. For HTTPS behind Caddy, allow the external hostname and proxy the whole site to the Prysm HTTP listener. Capture uses the same listener at `/api/capture`. [Docker setup](docker.md#network-and-deployment-options)

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
2. Open **Prysm instance URL** and enter a LAN host such as `192.168.1.10`, or the full panel URL such as `https://prysm.example.lan`. Choose **Use this address**.
3. Leave **Combined** and **LZ4** selected for the first run; choose **Save changes**.
4. Choose **Enable** (or **Save and enable**).

A bare host uses HTTP on port 8787. A full URL respects its scheme and port, including the standard HTTP and HTTPS defaults. Save verifies the capture handshake and remembers the resulting origin for both capture and the calibration QR code. **Receiver in use** means another TV has the session. The sender retries automatically.

Arrows move focus; OK opens or selects a setting. Save applies changes. Closing
the app leaves capture enabled. **Enable with TV** optionally enables boot
startup; leave it off until you have checked standby/wake and cold boot on your TV.

## 4. Start capture

Wake the TV and show active video. In Prysm's host panel at `http://localhost:8787`,
select **Screen sync** and turn the lights on.
**Success:** the TV reports **Capturing** and Prysm displays frames or updates the LEDs.

**Ready to capture** means video is available and Prysm is idle. **Waiting for video**
means capture is enabled and the TV has no active video source. Play a video or
select an active HDMI input. The TV connects automatically when video returns.
Choosing Off or WLED effects releases the TV capture resources. **Capturing** alone
does not guarantee fresh video; inspect Prysm's output too.

## Change settings or remove

**Video only** excludes on-screen menus. **Combined** includes them.
If a previous package saved an incompatible configuration, move
`/var/lib/prysm-capture/config.json` to a backup through your root TV access,
then relaunch the service and configure the sender in the TV app.

**Prometheus metrics** is optional and defaults off; saving it restarts an active
worker. See [monitoring](../developers/monitoring.md).

Before uninstalling, choose **Service options → Remove service setup… → Remove setup**,
then uninstall the TV app. This stops the sender and removes capture setup;
saved receiver settings remain. See [troubleshooting](troubleshooting.md#tv-sender).
