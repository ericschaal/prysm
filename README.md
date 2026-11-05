<p align="center">
  <img src="docs/images/hero.svg" alt="Prysm — LED lighting around a television." width="100%">
</p>

<p align="center">
  <strong>LED strips that follow your screen.</strong><br>
  Prysm captures video, calculates edge colors, and sends them to WLED.
</p>

## What you need

- **Capture device:** an HDMI capture card or the webOS capturer on a rooted LG TV.
- **Prysm application:** runs on Linux or macOS, including a Raspberry Pi.
- **LED renderer:** WLED with LED strips.

```text
HDMI → capture card → Prysm → WLED → LEDs
Rooted LG TV → webOS capturer → Prysm → WLED → LEDs
```

Both setups need a separate computer running Prysm.

## Get started

1. [Install Prysm](docs/users/README.md#1-install-the-prysm-application).
2. Connect a [capture card](docs/users/quickstart.md) or [webOS capturer](docs/users/webos.md).
3. [Configure WLED and start Screen sync](docs/users/configuration.md).

Download packages from [Releases](https://github.com/ericschaal/prysm/releases),
or pull the Docker image from `ghcr.io/ericschaal/prysm`.

## In action

<p align="center">
  <img src="docs/images/ambilight-action.webp" alt="Red and blue LED light follows an action scene." width="768">
</p>

<p align="center">
  <img src="docs/images/ambilight-music.webp" alt="LED light follows the colors of a music video." width="768">
</p>

## Control panel

Open the panel to turn lights on or off, switch between Screen sync and WLED
effects, and change settings.

<p align="center">
  <img src="docs/images/control-panel.jpg" alt="Prysm power control and Screen sync or WLED effects selection." width="520">
</p>

[Preview](docs/users/preview.md) shows the captured picture and LED colors.

<p align="center">
  <img src="docs/images/picture-preview.jpg" alt="Captured picture, detected black bars, and LED edge colors." width="520">
</p>

## Develop Prysm

[Build](docs/developers/README.md) · [Architecture](docs/developers/architecture.md) ·
[Debugging](docs/developers/debugging.md) · [Contributing](docs/developers/guidelines.md)

[All documentation](docs/README.md) · [Research](research/README.md) ·
[Apache 2.0 license](LICENSE)
