# Run Prysm

Prysm needs a Linux or macOS host and a video source. For physical lighting,
you also need a working WLED controller and LED strips on the same trusted network.
Desktop recording is not supported directly; local device input uses a capture card or camera.

| Your setup | Guide |
| --- | --- |
| Raspberry Pi / Debian arm64 | [Install the package](debian.md) |
| Linux or macOS, USB capture device | [Capture device → WLED](quickstart.md) |
| Rooted LG TV, no external capture card | [Set up the TV sender](webos.md) |
| No LEDs yet, or a saved video to test | [Desktop preview](preview.md) |

After setup: [settings and daily use](configuration.md) · [troubleshooting](troubleshooting.md).

Building an executable yourself? Use [developer setup](../developers/README.md#build),
then return to the guide for your hardware. The web panel is included in Prysm;
you do not need to run a separate frontend server.
