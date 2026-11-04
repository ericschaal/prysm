# Connect an HDMI capture card

You need an HDMI capture card supported by your Linux or macOS host, a video
source connected to the card, and [Prysm installed](README.md#1-install-the-prysm-application).
Prysm reads the card through V4L2 on Linux or AVFoundation on macOS.

## 1. Connect the card

Connect the video source to the card's HDMI input and the card's USB connection
to the Prysm host. If your card has HDMI passthrough, connect that output to the
screen. Check that your usual picture still reaches the screen.

## 2. Configure Prysm

Start Prysm if your installation does not already run it as a service:

```sh
prysm --config settings.json
```

Open the control panel and choose **USB capture device** under **Capture from**:

| Host | Capture device value | Access needed |
| --- | --- | --- |
| Linux | Prefer `/dev/v4l/by-id/...`; `/dev/video0` also works. | The user running Prysm needs access to the video device. The Debian service uses the `video` group. |
| macOS | AVFoundation unique ID from **System Information → Hardware → Camera**. | Grant camera access when prompted. |
| Docker on Linux | Device path inside the container, such as `/dev/video0`. | Add the [device and group mappings](docker.md#tv-and-usb-capture). |

Choose a resolution and frame rate the card supports. Start with 640 × 360 at
30 fps if supported. Linux capture needs YUYV, RGB3, or BGR3; an MJPEG-only mode
will not work.

Continue with [WLED and browser setup](configuration.md). The LEDs should follow
the captured picture when **Screen sync** is on. If the preview is blank or
capture fails, see [troubleshooting](troubleshooting.md).
