# Calibration

Calibration is two small pieces, not a session:

- **Colour profiles** are ordinary settings: `processing.profiles` (at most 32)
  and `processing.active_profile`, saved through `PUT /api/v1/settings` and
  applied at the next frame. The correction maths is in `prysm-core`
  ([correction.rs](../../crates/common/prysm-core/src/correction.rs)) and runs
  after shadow lift, before brightness, for the LEDs and the preview alike.
- **The pattern** the TV shows is engine state beside the target:
  `PUT /api/v1/calibration` with `{ pattern, uncorrected }`, cleared by
  `DELETE`. It needs Screen sync on (`409 not_processed` otherwise) and never
  starts it. While set, the processor runs without smoothing, and without the
  profile when `uncorrected` is true. It clears when the target leaves
  Screen sync, at shutdown, and 10 minutes after the last `PUT`.

There are no tokens, leases or locks. The TV app and the phone both follow
`State.calibration` on the state stream. Patterns and rules:
[portal contract](../../v1/surfaces/web-portal/HTTP-CONTRACT.md#calibration).

## Flow

1. The TV app (either package) turns Screen sync on and shows a QR code of
   `<portal>/#/calibrate`: its own portal (full) or the desktop portal (slim).
2. The phone wizard ([calibrate.tsx](../../v1/frontend/packages/web-portal/calibrate.tsx))
   creates a draft profile, makes it active, sets one pattern per step and
   saves slider changes live. Finish names the profile; Cancel deletes it and
   restores the previous one.
3. The TV plays the chosen pattern full screen from bundled H.264 fragments
   through Media Source Extensions ([patterns.ts](../../v1/frontend/packages/tv-ui/patterns.ts)).
   Back on the TV sends `DELETE` and leaves the lights on.

This is visual matching, not optical measurement.

## Checks

```sh
npm --prefix v1/frontend test
npm --prefix v1/frontend run dev:webos   # /demo.html also plays the phone's part
```

*Not yet verified on hardware:* pattern playback and colour matching on a TV.
