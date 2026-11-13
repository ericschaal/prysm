# Live preview

Live preview shows the picture Prysm captures with the colours it sends to each
edge of the strip. Use it to tell a capture problem from a lighting problem.

1. Turn on **Screen sync** on Home.
2. In **Settings**, choose **See the picture** on the Video source card (or
   **Check placement** on the Lights card). The address is `#/preview` on
   the portal.
3. Change a [picture setting](configuration.md#processing) in Settings and
   watch the result on the lights and in the preview.

What you see:

- The captured picture, scaled down, with each edge's LED colours around it.
- An outline of the area Prysm samples when it removes black bars.

If the picture moves but the LEDs look wrong, check WLED and the LED counts.
If the picture is missing or frozen, check the video source. While the lights
are off the preview shows nothing.

The preview updates at most 8 times a second; judge smoothness on the real
lights. It runs only while the page is open and visible. Closing it leaves
Screen sync running.

Change one setting at a time; start with brightness and smoothing. For colour
tint differences with an LG TV, use [Calibrate colours](calibration.md).
