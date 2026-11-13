# LG TV

The Prysm app (`io.prysm.tv`) captures the picture of a rooted LG webOS TV.
It comes in two packages; installing one replaces the other.

| Package | The TV… | You also need |
| --- | --- | --- |
| **Full** | captures, works out the colours, drives WLED and serves the web portal | WLED |
| **Slim** | captures and sends the picture to Prysm on a computer, which drives WLED | A computer running Prysm ([Debian](debian.md), [Docker](docker.md) or [TrueNAS](truenas.md)), WLED |

Choose **full** if you do not want another computer. Choose **slim** if you
already run Prysm on a computer or NAS.

> **Not yet verified on hardware.** Neither package has passed acceptance
> testing on a TV yet: install, capture, standby and wake, cold boot and
> removal are still to be tested. Expect rough edges.

## Before you start

You need:

- An LG webOS TV [rooted](https://www.webosbrew.org/rooting/) with the
  Homebrew Channel, root execution, and SSH access as `root`.
- A computer with a checkout of this repository, for the install helper.
- WLED on the same network.

> **Safety.** Prysm only installs its own app and service files. Never
> overwrite the TV's system partitions (kernel, rootfs, tvservice) while
> following these or any other instructions. A mistake there can make the TV
> unusable.

If the older **Prysm Capture** app (`io.prysm.capture`) is installed, remove
it first: in that app choose **Service options → Remove service setup… →
Remove setup**, then uninstall the app. Prysm 1.x does not detect or migrate
it. Stop PicCap or any other capture app too.

## 1. Install

Download `io.prysm.tv-full_VERSION_all.ipk` or `io.prysm.tv-slim_VERSION_all.ipk`
from a `v1.*` [release](https://github.com/ericschaal/prysm/releases), or
[build it](../developers/packaging.md#webos). From the repository root,
install it over SSH (replace `TV` with the TV's address; `--dry-run` only
prints the steps):

```sh
v1/packaging/webos/install-ipk.sh root@TV path/to/io.prysm.tv-full_VERSION_all.ipk
```

The helper copies the package to the TV, installs it through the TV's install
service and runs the service setup. `ares-install` does not work on webOS 10
("Unable to exec"); use the helper.

After every install or update the service must be set up again; the helper
does this. If you installed another way, open **Prysm** on the TV and choose
**Set up service**.

<a id="processing-on-this-tv-experimental"></a>

## 2a. Full package: the TV drives the lights

1. Open **Prysm** on the TV. If it says **Prysm needs setting up**, choose
   **Set up service**.
2. The TV shows the address of its web portal and a QR code. Open it on your
   phone or computer and run the [setup wizard](configuration.md). The video
   source is **This TV** (640 × 360 at 30 fps by default; **Include TV menus**
   decides whether on-screen menus light the LEDs).
3. Play video. Choose **Start screen sync** on the TV or turn on Screen sync
   in the portal.

The portal stays available while the TV is awake, also with the lights off or
the app closed. On the TV, the portal also accepts requests from web pages on
other sites, so keep the TV on a trusted network.

## 2b. Slim package: the TV sends the picture

<a id="slim-package-the-tv-sends-the-picture"></a>

1. On the computer, start Prysm with `--allow-origin null`, so the TV app may
   use its portal ([Debian](debian.md#manage-the-service),
   [Docker](docker.md#receive-the-picture-from-a-tv-slim-package),
   [TrueNAS](truenas.md)). Without this option the TV app says how to allow it.
2. In the computer's portal, choose the video source
   **Capture-only TV (remote)**. Keep **Hardware H.264 (recommended)** and
   320 × 180 at 30 fps to start.
3. On the TV, open **Prysm**. Set **Prysm address** to the computer's portal,
   for example `http://192.168.1.20:8787`. The TV sends its picture to that
   same address; nothing else needs to be reachable.
4. Choose **Start capture**, then turn on Screen sync in the portal or on the
   TV.

Behind an HTTPS reverse proxy, give the TV the proxy's address, such as
`https://prysm.example.com`: the picture then travels encrypted. The proxy
must pass WebSockets on `/api/v1/capture/stream` and must not put that path
behind a login, which the TV cannot do. The computer's LAN address is the
lowest-delay choice.

The TV captures only while the computer's lights are on; otherwise it shows
**Paused by Prysm**. Picture size, rate, menus and encoding are set in the
computer's portal, which sends them to the TV on each connection.

If the TV cannot encode the stream, Screen sync stops with the reason. Choose
**Software H.264** or **Uncompressed** under **Settings → Video source** in
the portal. Uncompressed needs about ten times the network.

## Daily use

- Closing the app leaves the service running. On Back, the app asks before
  closing.
- **Start with TV** starts Screen sync (full) or capture (slim) when the TV
  turns on. Turn it on only after you have tested standby, wake and a cold
  boot.
- **Service options** has **Set up or repair service**, **View diagnostics**
  and **Remove service setup…**.
- [Calibrate colours](calibration.md) from the app.

TVs older than webOS 5 can run capture, but the app cannot control a portal;
use the portal on a phone or computer instead.

For problems, see [troubleshooting](troubleshooting.md#lg-tv).

## Remove

1. In the app: **Service options → Remove service setup… → Remove setup**.
   Capture stops and the service registration and autostart are removed.
2. Then uninstall the app.

Settings in `/var/lib/io.prysm.tv` on the TV stay. Removing the app first can
leave the service registered, because the package has no removal script.
