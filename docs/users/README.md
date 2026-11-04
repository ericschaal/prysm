# Set up Prysm

| Part | What you need |
| --- | --- |
| **Capture device** | HDMI capture card, or the **webOS capturer** on a rooted LG TV. |
| **Prysm application** | Linux or macOS computer, including a Raspberry Pi. This is the **Prysm host**. |
| **LED renderer** | WLED controller and LED strips. |

```text
HDMI → capture card → Prysm host → WLED → LEDs
Rooted LG TV → webOS capturer → Prysm host → WLED → LEDs
```

Both setups need a Prysm host. Keep all devices on a trusted LAN; Prysm has no login.

## 1. Install the Prysm application

Use the [latest release](https://github.com/ericschaal/prysm/releases):

| Host | Guide |
| --- | --- |
| Raspberry Pi / Debian arm64 | [Debian package](debian.md) |
| Docker | [Docker](docker.md) |
| TrueNAS | [TrueNAS](truenas.md) |
| Other Linux or macOS | [Build from source](../developers/README.md#build) |

## 2. Connect your capture device

- [HDMI capture card](quickstart.md)
- [webOS capturer](webos.md)

Prysm cannot capture your computer's desktop directly.

## 3. Set up your lights

[Connect WLED and start Screen sync](configuration.md) in the browser.

Then: [Preview and tuning](preview.md) · [Color calibration](calibration.md) ·
[Troubleshooting](troubleshooting.md).
