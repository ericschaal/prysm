# Connect an HDMI capture card

You need [Prysm installed](README.md#1-install-the-prysm-application) on a
Linux or macOS computer.

*Not yet verified on hardware:* Prysm 1.x has not been tested with a real
capture card on Linux or macOS.

1. Connect the video source (console, set-top box) to the card's HDMI input.
2. Connect the card to the computer by USB.
3. If the card has an HDMI output (passthrough), connect it to the screen.

## Choose the card in Prysm

Open the web portal. In setup, or later under **Settings → Video source**,
choose **Video capture device**. Select **Find devices** to list the cards
Prysm can see, or type the device yourself:

| Computer | Device value |
| --- | --- |
| Linux | `/dev/video0`, or a stable `/dev/v4l/by-id/...` path. Prysm's user needs access to the device (the `video` group). |
| macOS | The device's unique ID. **Find devices** fills it in. Allow camera access when macOS asks. |
| Docker on Linux | The path mapped into the container, such as `/dev/video0`. See [Docker](docker.md#use-a-capture-card). |

Leave the picture at 640 × 360 and 30 fps to start; the card must support the
mode you choose. On Linux the card must offer YUYV, RGB3 or BGR3; cards that
only offer MJPEG do not work.

Continue with [setting up your lights](configuration.md).
