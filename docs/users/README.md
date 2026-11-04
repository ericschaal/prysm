# Set up Prysm

Prysm makes the LED strips behind your screen follow the picture. There are
three parts to the setup:

| Part | What it does | Your choices |
| --- | --- | --- |
| **Capture device** | Captures the video you are watching. | An HDMI capture card, or the Prysm webOS capturer on a rooted LG TV. |
| **Prysm application** | Reads the captured video, calculates the LED colors, and provides the browser control panel. | Runs on a Linux or macOS computer, called the **Prysm host**. A Raspberry Pi can be the host. |
| **LED renderer** | Receives colors from Prysm and drives the LED strips. | **WLED** is the only supported renderer. |

With a capture card, Prysm captures through V4L2 on Linux or AVFoundation on
macOS. With an LG TV, the **webOS capturer** captures on the TV and sends the video
to Prysm over the network. You still need a Prysm host.

```text
HDMI video → capture card → Prysm host → WLED → LED strips
                              ↑
                      Browser control panel

Rooted LG TV → webOS capturer → Prysm host → WLED → LED strips
```

Keep Prysm, WLED, and the webOS capturer on a trusted local network. The control
panel and TV connection have no login.

## 1. Install the Prysm application

Prysm has no official release yet. Use a current CI artifact or build it yourself.
Choose where Prysm will run:

| Prysm host | Installation guide |
| --- | --- |
| Raspberry Pi or Debian arm64 | [Install the Debian package](debian.md) |
| A computer running Docker | [Run with Docker](docker.md) |
| TrueNAS | [Install a TrueNAS app](truenas.md) |
| Other Linux or macOS computer | [Build Prysm](../developers/README.md#build), then run `prysm` |

The browser panel is included. You do not need a separate web server.

## 2. Connect your capture device

- **HDMI capture card:** follow [capture card setup](quickstart.md).
- **Rooted LG TV:** follow [webOS capturer setup](webos.md). This replaces the capture
  card; you still need the Prysm application and WLED.

Prysm does not record your computer's desktop directly. A camera can also be a
USB capture device; a [video file](configuration.md#video-files) is useful for testing.

## 3. Set up your lights

Follow [Settings and daily use](configuration.md) to connect WLED, enter the LED
layout, and start **Screen sync**. No configuration file needs to be written by hand.

Once the lights follow the picture, you can [preview and tune it](preview.md),
[calibrate colors with an LG TV](calibration.md), or [fix a problem](troubleshooting.md).
