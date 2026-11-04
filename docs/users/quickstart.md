# Connect an HDMI capture card

You need [Prysm installed](README.md#1-install-the-prysm-application) on Linux or macOS.

1. Connect the video source to the card's HDMI input.
2. Connect the card to the Prysm host by USB.
3. If the card has HDMI passthrough, connect its output to the screen.

## Configure Prysm

Start `prysm` if it is not already running as a service. Open its browser panel
and choose **USB capture device** under **Capture from**.

| Host | Capture device value |
| --- | --- |
| Linux / V4L2 | `/dev/video0`, preferably a stable `/dev/v4l/by-id/...` path. The Prysm user needs video-device access. |
| macOS / AVFoundation | Unique ID from **System Information → Hardware → Camera**. Grant camera access when prompted. |
| Docker on Linux | Mapped path, such as `/dev/video0`. Add the [device and group mapping](docker.md#tv-and-usb-capture). |

Choose a resolution and frame rate the card supports. Try 640 × 360 at 30 fps.
Linux capture requires YUYV, RGB3, or BGR3; MJPEG-only modes do not work.

Continue with [WLED setup](configuration.md).
