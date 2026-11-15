# Calibrate colours

Calibrating saves a **calibration**: it matches the lights’ colours on the wall
to the colours on the TV. You match by eye: the TV shows test patterns and you
adjust sliders on your phone.

It needs the Prysm app on an LG TV ([full or slim package](webos.md)), working
Screen sync from that TV, and a phone on the same network.

*Not yet verified on hardware:* the TV pattern player and the phone wizard
have not been tested on a TV with Prysm 1.x.

## Run it

1. On the TV, open **Prysm** and choose **Calibrate colours**. Screen sync
   turns on and the TV shows a QR code. If the row is greyed out, the TV says
   why (for example, the lights are not using this TV's picture).
2. Scan the QR code with your phone. It opens the portal's Calibrate page.
   You can also open **Settings → Calibrations → New calibration** in the
   portal.
3. Start a new calibration, or start from a copy of a saved one.
4. Follow the steps. Each shows a pattern on the TV and asks one question.
   If it already looks right, tap **Looks right**. The next step shows at
   once; its swatch beside "On your TV" stays dim until the TV shows the
   pattern. If the TV does not change, the phone goes back to the step the TV
   shows and says why.
   - **White:** make the wall as neutral as the grey on the TV. One slider
     goes from **Cooler** to **Warmer**, the other from **More green** to
     **More pink**. Ignore brightness.
   - **Red, Green, Blue:** compare each half of the TV with the wall. The
     first slider turns the colour (for red: **More pink** or **More
     orange**), the second makes it **Softer** or **Stronger**. Adjusting a
     colour does not change white.
   - **Checks:** yellow, cyan, magenta and three mixed scenes. If one does not
     match, tap the colour it links to (for yellow: **Adjust red** or
     **Adjust green**); **Looks right** there brings you back to the check.
   - **Review:** name the calibration and finish.

   **Reset** puts the sliders on the screen you are on back to 0.
5. Press Back on the TV to leave. The lights stay on.

Changes show on the wall as you move a slider; the line under each slider
says which way to move it. **Show without correction** compares the wall
without the calibration; it shows a tick while it is on.

**Cancel calibration** (top right) deletes the new calibration and brings
back the one you used before. If you changed anything, it first asks
**Discard your changes?** in the same line: **Keep** carries on, **Discard**
cancels. If calibrating ends on the TV side, the phone offers to resume and
keeps your draft. A pattern left on the TV clears itself after 10 minutes.

## Use calibrations

- **Home:** tap a calibration to use it, or **No correction**.
- **Settings → Calibrations:** choose a calibration to rename it, adjust its
  colours (the same sliders as when calibrating, plus **Midtones**) or delete it; make a new one with **New calibration**.

Calibrations do not change brightness, smoothing or other picture settings,
and do not switch with the TV's input. Check the result with the content you
usually watch.
