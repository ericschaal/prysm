# Calibrate Colors

Use this visual wizard to match the tint of reflected wall light to your TV.
Start with the TV and WLED powered, the webOS sender connected, and Prysm
configured with the webOS input. Keep the phone on the receiver's local network.

1. Open Prysm on the TV and choose **Calibrate Colors** under **Settings**.
2. Check the Prysm web address, choose **Show QR code**, and scan the QR code.
   Before pairing, **Back** returns to the address field.
3. The TV waits while you choose **New profile** or select a saved profile on
   your phone. On the response check, the TV starts cycling through gray, red,
   green, and blue. Confirm that the wall lights follow the sequence.
4. Match the light-gray tint using Cooler/Warmer and Greener/More magenta.
   Judge tint separately from brightness. If an older profile has a dark-tone
   adjustment, you can keep it or reset it on this screen.
5. In **Match color intensity**, compare the moderate blue on the left and the
   vivid blue on the right with the wall behind each half of the TV. Try the
   red and green pairs too. Adjust only if the change improves both sides.
   Choose **Next** to check individual hues.
6. In **Fine-tune individual hues**, choose **Skip hue adjustment** to keep
   your settings. To adjust a color, tap its swatch. Blue keeps the same split-screen comparison. If the wall blue
   looks too purple, move Blue hue toward Cyan. If it looks too teal, move it
   toward Magenta. Leave hue alone if the only difference is brightness.
   **Next** returns to mixed colors.
7. In **Check colors together**, choose Coast, Garden, or Dusk. Compare each
   half of the TV with the corresponding wall edge using your everyday
   processing settings. You can revisit intensity or individual hues here.
8. Compare each corner of the four-color grid with the glow behind it.
   Name the profile and choose
   **Save and use**. When the TV shows **Calibration finished**, press Return
   on the remote to exit.

Hold **Hold for uncorrected** to compare with all profile correction bypassed.
Release it to return to the draft. Brightness, smoothing, shadow lift and
spatial reach are independent settings and are never stored in a profile.

In Overview, tap **Color profile** beneath **Screen sync** to apply a saved
profile or **No correction** immediately. To manage profiles, open **Settings →
Color profiles** and use the **•••** button beside a name. Rename inline or mark
a profile for deletion. **Undo** restores that row. **Save & apply** commits
your edits together, and **Discard** restores the saved profiles. At the
32-profile limit, adjust an existing profile or delete one before starting a new one. Deleting
the active profile selects No correction. Labels such as PS5 and Freebox do
not automatically switch with the HDMI source.

Back/Exit on the TV discards unsaved changes. Stop lighting always cancels
calibration and leaves the lights off. A lost phone or TV connection has a
30-second reconnection window before cancellation. Temporary reference
processing is restored on exit; the TV stays in Prysm without switching input.

This is visual matching, not a measured calibration. The shared midtone curve
cannot fix a tint that changes with brightness, and different picture modes
may need different profiles. Check the result with your usual content afterward.
