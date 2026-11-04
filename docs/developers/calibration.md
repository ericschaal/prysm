# Calibration

The controller owns one TV/phone session and saves profiles through the existing
revision-checked atomic settings store. Entry points:
[session/controller](../../apps/prysm/runtime/src/control/calibration.rs),
[phone hook](../../crates/web/prysm-web/control-ui/useCalibrationSession.ts).

## Session rules

- Pairing is single-use, expires in two minutes and issues separate TV/phone
  capabilities. Keep tokens out of public state and URLs sent to servers.
- Heartbeats allow 30 seconds to reconnect. Slider updates use monotonic sequence
  numbers; retry the latest unacknowledged update and wait for it before Save.
- Uncorrected comparison uses a two-second renewable lease.
- Block full-settings writes/powered source changes during the session. Stop,
  expiry and capture failure discard drafts and restore saved processing.
- Save requires running capture. Failed persistence keeps the draft editable;
  cleanup failure after persistence must not report it unsaved.
- Cancellation joins startup and accepted WLED writes before Stop, preventing late On.

## Color and patterns

Correction runs after shadow lift, before brightness; preview/output share it.
Profiles hold warmth, tint, midtones, intensity and six hues, not capture or
brightness. Math lives in [correction.rs](../../crates/common/prysm-core/src/correction.rs).
Reference steps use 70% brightness, zero smoothing/shadow lift. Mixed/Review restore
normal processing with the draft. This is visual matching, not optical measurement.

[calibration-patterns.json](../../crates/web/calibration-patterns.json) owns swatch/clip
order. TV playback uses MSE fragments with bundled MP4 fallback to keep capture
active. `/calibration-tv.html` relays only its verified direct file-origin parent;
retain normal API access checks. Pairing uses a URL fragment.

## Local checks

From `crates/web/prysm-web/control-ui`:

```sh
npm run dev:calibration
npx playwright install chromium webkit
npm run test:browser
npm test
```

Demo: `http://127.0.0.1:5180/#/calibrate/local-demo`, in-memory only.
Regenerate/test both frontends after contract changes. Browser tests cover layouts,
rejoin, leases, updates, failed saves and Stop; validate TV decoding/colors on hardware.
