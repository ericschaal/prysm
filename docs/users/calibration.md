# Calibrate colors with an LG TV

Use this visual wizard if the wall light has a different tint from your TV.
It needs the [webOS capturer](webos.md), WLED, and working Screen sync. Keep your
phone on the same network as the Prysm host.

## Match the colors

1. On the TV, open Prysm and choose **Settings → Calibrate Colors**.
2. Scan the QR code with your phone. It opens calibration on your Prysm host.
3. Choose **New profile** or a saved profile. In the response check, confirm
   that the wall lights follow the TV's gray, red, green, and blue sequence.
4. Match the light-gray tint with **Cooler/Warmer** and **Greener/More magenta**.
   Judge tint separately from brightness.
5. In **Match color intensity**, compare each half of the picture with the wall
   behind it. Try the blue, red, and green pairs. Keep a change only if it improves
   both halves, then choose **Next**.
6. In **Fine-tune individual hues**, choose **Skip hue adjustment** unless a
   color's tint is wrong. Tap a swatch to adjust it. For example, move Blue hue
   toward Cyan if the wall looks too purple, or toward Magenta if it looks too
   teal. Leave hue alone for a brightness difference.
7. In **Check colors together**, try Coast, Garden, and Dusk. Compare each half
   with its wall edge, then check the corners of the four-color grid.
8. Name the profile and choose **Save and use**. When the TV shows
   **Calibration finished**, press Return to exit.

Hold **Hold for uncorrected** to compare without the profile's corrections.
Release it to return to your draft. Brightness, smoothing, shadow lift, and
spatial reach are separate picture settings and are not stored in a profile.

## Use and manage profiles

In **Overview**, tap **Color profile** beneath **Screen sync** to apply a saved
profile or **No correction**. Different TV picture modes may need different
profiles. Profile names do not switch automatically with the HDMI source.

Under **Settings → Color profiles**, use **•••** beside a profile to rename or
mark it for deletion. **Undo** restores a deleted row. Choose **Save & apply**
to commit changes or **Discard** to cancel them. Deleting the active profile
selects No correction. You can keep up to 32 profiles.

## Exit or cancel

Back/Exit on the TV discards unsaved changes. **Stop lighting** cancels
calibration and leaves the lights off. A lost phone or TV connection has
30 seconds to reconnect before cancellation. Prysm restores the picture
processing used before calibration when you exit.

This is visual matching against your TV and wall. Check the saved result with
your usual content; one profile cannot correct every brightness-dependent tint.
