# Visual calibration

Calibration matches LED wall light to the TV by eye. It does not measure panel
emission, wall reflectance or HDR transfer. TV capture remains limited-range
BT.709; do not infer tone mapping from HDMI metadata.

## Ownership and persistence

The controller owns one temporary session. `Session` owns authorization, expiry,
draft ordering, comparison leases and save eligibility; its transitions return
runtime/persistence work to the controller. Settings stores up to 32 profiles,
with 64-character names and integer controls from -100 to 100: warmth, tint,
midtones, intensity and six hue anchors. Capture, brightness, smoothing and
source are separate settings. No correction is always available.

The TV starts a session with single-use pairing valid for two minutes. TV and
phone receive separate random capabilities; the phone edits, either owner cancels.
Thirty-second heartbeat leases allow reconnect. The phone keeps its capability in
session storage, not a recoverable draft. Restart loses the session. Other clients
see only a lock flag. If a pairing reply is lost, start a fresh link rather than
reusing the consumed capability.

`useCalibrationSession` owns requests, token storage, one-second polling, ordered
updates and comparison release. Temporary network errors retry; rejected/expired
tokens stop retries and clear storage. Slider updates carry a monotonic sequence;
the latest unacknowledged update is retried. Save waits for acknowledgment.
Hold for uncorrected bypasses the profile using a two-second renewable lease,
so a lost release cannot leave correction bypassed.

Reference steps temporarily use 70% brightness, no smoothing and no shadow lift.
Spatial settings remain unchanged. Mixed colors/Review restore normal processing
with the draft. Slider changes never persist settings.

Full-settings writes and powered source changes are blocked during calibration.
Stop cancels the session, joins capture and restores saved processing. Exit,
disconnect expiry and capture failure discard the draft and stop lighting.
These paths do not switch TV inputs. Save uses the revision-checked atomic settings
store and activates the profile. A failed save leaves the draft editable; cleanup
failure after persistence must not report the profile as unsaved.

Startup is a controller job so heartbeats and Stop remain responsive. Cancellation
finishes accepted WLED writes, then joins startup before stopping output, preventing
a late On. Draft updates apply when startup completes; Save requires running capture
and stays unavailable during frame loss.

## Correction math and placement

Correction acts on sampled/smoothed linear colors after shadow lift, before
brightness. Preview and WLED consume the same result; WLED gamma is unchanged.

| Control | Operation |
| --- | --- |
| Midtones `m` | Map peak `p` to `p / (p + (1-p) * 2^(-m/100))`, scaling channels together. Black/white stay fixed and channel ratios are preserved. |
| Intensity | Scale chroma around linear BT.709 luminance. |
| Hues | Shift six RGB wheel anchors toward neighbors by up to half the distance; interpolate between anchors. |
| Warmth `w`, tint `t` | Apply `[2^(w/200), 2^(-t/200), 2^(-w/200)]`, normalized by its maximum (at least one), to attenuate without clipping highlights. |

Intensity/hues sit between the tone curve and white balance. A common chroma limit
keeps values in gamut at the original luminance; neutrals bypass chroma changes.
Identity correction returns unchanged colors.

## TV transport and patterns

The phone, TV and generator share [calibration-patterns.json](../../crates/web/calibration-patterns.json).
Keep swatches, clip names and cycle order there. The TV uses half-second H.264
fragments in one Media Source Extensions player, falling back to bundled MP4s.
Static patterns loop; cycle patches last two seconds. A queued-pattern indicator
clears when playback reaches the requested patch. Both paths keep video active
for capture. Profile selection leaves decoding idle until startup needs a pattern.

`/calibration-tv.html` relays Poll/Cancel from its direct file-origin TV parent.
Only this page permits `frame-ancestors file:`; verify the parent window and
webOS file origin. The same-origin API retains normal Host, Origin, Fetch Metadata
and size checks. Pairing uses a QR URL fragment, keeping capabilities out of
ordinary requests/referrers. Use the verified instance origin for the relay and QR.

The phone uses HashRouter at `/#/calibrate/:pairing`. Joining and loading settings
must both finish before showing the wizard. Main navigation retains the Settings
draft; preview follows the active route. The flow is Profile → Response → White
balance → Intensity → Hues → Mixed colors → Save. Hues can be skipped without
losing the draft. Keep reference colors digital targets, separate from optical
claims about the wall.

## Local checks

From `crates/web/prysm-web/control-ui`:

```sh
npm run dev:calibration
npx playwright install chromium webkit
npm run test:browser
npm test
```

The demo opens at `http://127.0.0.1:5180/#/calibrate/local-demo` with an in-memory
API: no TV, WLED or persistent settings. Browser tests build both UIs and cover
mobile/narrow layouts, updates, rejoin, leases, save failures and Stop. Screenshots
and traces go to ignored `test-results/`.

When changing contracts, regenerate and test both UIs. Rust/DOM checks also cover
identity/ramps, pairing reuse, stale updates, locks and profile changes. Simulation
does not establish optical accuracy or TV decoding; validate on target hardware.
