# Set up Prysm

Prysm reads the picture from a video source, works out a colour for each LED
around the screen, and sends those colours to a [WLED](https://kno.wled.ge/)
controller.

## Choose a setup

| Setup | Picture comes from | Prysm runs on | You need |
| --- | --- | --- | --- |
| **Capture card** | An HDMI capture card | A Linux or macOS computer, such as a Raspberry Pi | Capture card, computer, WLED |
| **LG TV, full package** | The TV itself | The TV | Rooted LG webOS TV, WLED |
| **LG TV, slim package** | The TV itself | A computer on your network; the TV only sends the picture | Rooted LG webOS TV, computer, WLED |

```text
HDMI → capture card → computer running Prysm → WLED → LEDs
LG TV running Prysm (full) → WLED → LEDs
LG TV (slim) → computer running Prysm → WLED → LEDs
```

Prysm cannot capture a computer's own desktop.

Keep every device on a trusted home network. The Prysm web portal has no
password: anyone who can reach it can control the lights.

> **Status.** Prysm 1.x has not yet been verified on real hardware: capture
> cards, WLED devices and both TV packages are still to be tested end to end.
> Steps marked *not yet verified on hardware* may change.

## 1. Install the Prysm application

For the **capture card** or **slim TV** setup, install Prysm on a computer:

| Computer | Guide |
| --- | --- |
| Raspberry Pi OS 64-bit, Debian 12+, Ubuntu 22.04+ | [Debian package](debian.md) |
| Any Linux with Docker | [Docker](docker.md) |
| TrueNAS SCALE | [TrueNAS](truenas.md) |
| macOS, or Linux from source | [Build from source](../developers/README.md#build-and-run) |

Packages come from the
[GitHub releases](https://github.com/ericschaal/prysm/releases) tagged `v1.*`.

For the **full TV** setup, skip this step and go to [LG TV](webos.md).

## 2. Connect the picture

- [HDMI capture card](quickstart.md)
- [LG TV](webos.md) (full or slim package)

## 3. Set up your lights

Open the web portal and follow the [setup wizard](configuration.md).

Then: [Live preview](preview.md) · [Calibrate colours](calibration.md) ·
[Troubleshooting](troubleshooting.md).
