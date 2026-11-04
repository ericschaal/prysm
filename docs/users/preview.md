# Preview and tune the picture

Preview shows what the Prysm application receives from your capture device,
the detected picture boundaries, and the colors calculated for each LED edge.
Start with [Screen sync working](configuration.md); WLED is required even when
you are inspecting the preview.

![Prysm browser preview and picture settings](../images/preview-tuning.jpg)

## Open the preview

1. In **Overview**, choose **Screen sync** and turn the lights on.
2. Choose **Preview**, then **Open preview**.
3. Check that the image matches your video and changes as it plays.

The dashed outline shows the picture area used after black-bar removal. Edge
colors show what Prysm calculates for the lights. If the picture is correct but
the LEDs are wrong, check WLED, the LED counts, and the wiring direction.

The preview is a small, slower view of capture: at most 480 pixels across and
8 updates per second. Use the physical lights to judge smoothness. If capture
stops or a file finishes, the last image stays visible with a label.

## Tune the lights

Open **Settings**, change one [picture setting](configuration.md#processing),
and choose **Save & apply**. Watch the preview and wall light while comparing.
Use **Discard** to return to saved values before saving.

Start with brightness and smoothing. Adjust shadow lift if dark scenes need
more light. Leave sample density at 128 unless increasing it visibly improves
the result. For an LG TV, [color calibration](calibration.md) can help match tint.

**Pause** or **Close** stops preview requests; hidden browser tabs pause them too.
Opening or closing Preview does not start or stop Screen sync. Closing the
browser leaves Prysm running.
