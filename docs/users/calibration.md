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
4. Follow the steps. Each shows a pattern on the TV:
   - **White balance:** make the wall as neutral as the grey on the TV with
     **Warmth** and **Tint**. Ignore brightness.
   - **Hues:** change a colour only if the wall leans towards a neighbouring
     colour.
   - **Intensity:** compare each half of the TV with the wall behind it.
   - **Mixed colours:** check that colours stay distinct side by side.
   - **Review:** name the calibration and finish.
5. Press Back on the TV to leave. The lights stay on.

Changes show on the wall as you move a slider. A button compares the wall
without the calibration.

**Cancel** deletes the new calibration and brings back the one you used
before. If calibrating ends on the TV side, the phone offers to resume and
keeps your draft. A pattern left on the TV clears itself after 10 minutes.

## Use calibrations

- **Home:** tap a calibration to use it, or **No correction**.
- **Settings → Calibrations:** choose a calibration to rename it, adjust its
  colours or delete it; make a new one with **New calibration**.

Calibrations do not change brightness, smoothing or other picture settings,
and do not switch with the TV's input. Check the result with the content you
usually watch.
