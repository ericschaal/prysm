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
choose **Capture card or camera**. Prysm lists the cards it can see; tick
yours (**Look again**, beside the heading, after plugging one in). If yours is not listed, enter it
in the last row, under the cards found:

| Computer | Device value |
| --- | --- |
| Linux | `/dev/video0`, or a stable `/dev/v4l/by-id/...` path. Prysm's user needs access to the device (the `video` group). |
| macOS | The device's unique ID. Ticking the card fills it in. Allow camera access when macOS asks; without it the card does not open ([troubleshooting](troubleshooting.md#picture-and-lights)). |
| Docker on Linux | The path mapped into the container, such as `/dev/video0`. See [Docker](docker.md#use-a-capture-card). |

**Size and rate** appears once a card is ticked and offers only what the card
delivers, each with the size and rate it sets: keep **Balanced** (640 × 360
at 30 fps, or the card's nearest larger size) to start. **Smooth** (60 fps)
shows only for a card that captures 60 fps; **Efficient** (320 × 180 at 30
fps) is less work for the computer. While the card is unplugged its size and
rate stay as saved and cannot be changed; once it is plugged in, choose
**Try again** under them. On Linux the card must offer YUYV,
RGB3 or BGR3; cards that only offer MJPEG do not work. A card with nothing
Prysm can use, such as one that only captures at 25 or 50 fps, shows why
under the cards instead of **Size and rate**.

Continue with [setting up your lights](configuration.md).
