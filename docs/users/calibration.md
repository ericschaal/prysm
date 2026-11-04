# Match your lights

Use this visual wizard to match the tint of reflected wall light to your TV.
Start with the TV and WLED powered, the webOS sender connected, and Prysm
configured with the webOS input. Keep the phone on the receiver's local network.

1. Open Prysm on the TV and choose **Match your lights**.
2. Check the Prysm web address, choose **Show QR code**, and scan the QR code.
   Before pairing, **Back** returns to the address field.
3. The TV waits while you choose **New profile** or select a saved profile on
   your phone. On the response check, the TV starts cycling through gray, red,
   green, and blue. Confirm that the wall lights follow the sequence.
4. Match the light-gray tint using Cooler/Warmer and Greener/More magenta.
   Compare tint, not brightness. Reset returns that step to neutral.
5. In **Match color intensity**, try Clay, Leaf, and Denim. Increase intensity
   if the wall looks washed out and decrease it if the colors look too vivid.
   Use a setting that works for all three.
6. In **Fine-tune individual hues**, select a static color that looks off.
   The slider names the neighboring colors it moves toward. For example, if
   red looks orange on the wall, move Red hue toward Magenta. Each slider
   affects that color and nearby mixtures. **Cycle colors** checks all six.
   Optional white-balance and dark-tone controls are also available here.
7. In **Check colors together**, choose Coast, Garden, or Dusk. Compare each
   half of the TV with the corresponding wall edge using your everyday
   processing settings. Revisit hue or intensity if a change helps one
   patch but makes another look worse.
8. Review with your usual processing settings, name the profile, and choose
   **Save and use**. Press Back on the TV when finished.

Hold **Hold for uncorrected** to compare with all profile correction bypassed.
Release it to return to the draft. Brightness, smoothing, shadow lift and
spatial reach are independent settings and are never stored in a profile.

Choose the active profile in Overview, including **No correction**. In
Settings you can rename or delete profiles, then save your settings. At the
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
