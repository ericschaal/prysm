# Visual calibration

Calibration matches the color character of LED wall light to the TV by eye. It
does not measure panel emission, wall reflectance, or a source HDR transfer
function. Capture remains limited-range BT.709; do not add HDMI-metadata-based
tone mapping. See the [capture evidence](../../research/webos-capture/2026-11-03-calibration-playback/README.md).

## Ownership and persistence

The existing controller owns one temporary session and all settings mutations.
`Session` keeps its fields private and owns authorization, pairing expiry, draft
ordering, comparison leases, profile selection, and save eligibility. Its
transitions return the runtime or persistence work for the controller to perform.
Saved and discarded outcomes are distinct; a runtime cleanup error cannot make a
persisted profile appear unsaved.
`Settings.calibration` defaults to an empty profile list and no active profile
for older settings. Each profile stores an id, name, and bounded integer controls
(-100…100) for warmth, tint, midtones, intensity, and six hue anchors. Older
profiles default the new controls to zero and serialize as before. It contains no brightness,
smoothing, shadow lift, spatial reach, capture, or source associations. Up to
32 profiles and 64 characters per name are allowed. The Overview selector
activates a profile, or the permanent No correction option. Settings stages
rename/deletion through its existing complete-document save; deleting the
active profile selects No correction.

A session starts on the TV. Pairing lasts at most two minutes and is single-use.
TV and phone receive separate random UUID capabilities; the phone may edit,
while either owner may cancel. Heartbeats allow 30 seconds for reconnection.
The phone keeps only its capability in session storage, not a recoverable draft.
A restart loses the session and capabilities. Controller state exposes only a
lock flag to other clients, not pairing or owner tokens.

The phone's `useCalibrationSession` hook owns requests, capability storage,
polling, ordered updates, and comparison release. The wizard owns presentation
and form fields. Initial pairing and stored-token rejoin retry temporary network
and server failures at the normal one-second interval. Rejected or expired
capabilities stop retries, clear storage, and require a fresh TV link. Pairing
remains single-use: if the server accepted pairing but its response was lost,
the next attempt is rejected and a fresh link is needed.

Polling is intentional for one TV and one phone during a short calibration.
Adjustments send immediately; TV step changes have roughly one-second latency.
WebSockets would be warranted if faster TV synchronization becomes a requirement.

Reference steps temporarily use **70% brightness**, zero smoothing and zero
shadow lift. Spatial settings stay as configured. Mixed and Review restore normal
processing with the draft correction. Returning to a reference step restores its processing.
Slider updates never persist settings. A monotonic update sequence rejects old
slider responses. The phone retains the latest unacknowledged update and retries
it after polling; Save waits until that update is acknowledged. Hold for uncorrected bypasses the complete profile with a
two-second renewable lease so a lost release cannot leave correction bypassed.

Every full-settings write and powered source change is rejected while the
session is active, with `Calibration in progress—finish or exit on the TV to
change settings.` Stop cancels the session, drains capture, restores the saved
processing configuration, and stays stopped. Exit, disconnect timeout, and
capture failure discard the unsaved correction and stop lighting. These paths
never switch TV inputs. A successful Save and use goes through the existing
revision-checked atomic settings store and activates the profile; persistence
failure keeps the draft editable. The saved review video remains until TV Exit
so saving itself does not replace the approved picture.

Startup runs as a controller job so heartbeats and Stop remain available while
WLED readiness is pending. Cancellation finishes any accepted WLED write, then
joins startup before stopping output, preventing a late On from undoing Stop.
Draft updates received during startup apply when it completes; Save is rejected
until startup finishes.

## Correction math and placement

The processor applies correction to the sampled/smoothed linear edge colors,
after the independent shadow lift and before independent brightness scaling.
Preview and output consume the same corrected edge colors. WLED gamma and
configuration are untouched.

For warmth `w` and tint `t`, the preliminary RGB gains are
`[2^(w/200), 2^(-t/200), 2^(-w/200)]`, divided by their maximum (at least one).
This changes tint by attenuation without clipping highlights. For shared
midtones `m`, the peak channel `p` maps to
`p / (p + (1-p) * 2^(-m/100))`; all channels scale together before the white
balance gains. Black and peak white are fixed by the tone curve, which is
monotonic and preserves channel ratios. Identity returns unchanged colors.
Between the tone curve and white balance, intensity scales chroma around
linear BT.709 luminance. Hue controls shift the six RGB wheel anchors toward
their immediate neighbors by up to half the distance. A color between two
anchors blends their adjustments. A shared chroma limit keeps channels in
gamut at the original luminance. Neutral inputs bypass both controls.

## TV transport and patterns

The TV app plays bundled MP4 assets, with the phone controls on a separate
screen. Gray and individual-color clips loop continuously and change only on
request; **Cycle colors** selects the six-color sequence. The existing `paused`
field selects a static color clip at `shade` (red, green, blue, cyan, magenta,
yellow) during the Colors step. It does not pause the video decoder: capture
still needs active video. Playback must activate the native video path; standalone HTML fills are
not sufficient. Asset generation and verification are documented with the
[capture experiment](../../research/webos-capture/2026-11-03-calibration-playback/README.md).

A tiny classic-script page at `/calibration-tv.html` relays only Begin, Poll,
and Cancel from its direct file-origin parent to the same-origin API. That page
alone allows `frame-ancestors file:`. It recognizes webOS's named file origins
as well as opaque local-file origins. Other web pages cannot frame it, and
unrelated windows cannot send commands through it. The API retains the normal
Host, Origin, Fetch Metadata and body-size protections; no CORS exception is
added. The phone's QR URL uses a fragment so the pairing capability is not sent
in ordinary URL requests or referrers. The TV suggests port 8787 and lets users
enter a different allowed web address before pairing.

Build host and TV frontends together when changing the generated calibration
contract. Unit and DOM tests cover correction ramps, identity, locks, pairing
reuse, stale updates, save failure, profile replacement/deletion, comparison
release, playback cleanup and timeout/Stop behavior. Physical validation is
still required across TV models and PS5/Freebox picture modes.

## Phone flow and local testing

The main path is Profile → Response → White balance → Intensity → Hues → Mixed colors → Save. Dark-tone
response remains available behind an optional disclosure in Colors. Existing
profile values are preserved, including a nonzero midtone correction. Neutral
matching targets tint, not equal luminance across a TV and reflected wall.
Intensity uses three muted single-color patches. Hues uses the six individual
clips and a cycle. Mixed colors uses three split-field clips to exercise both
wall edges with colors outside the adjustment set.

React Router owns navigation with a HashRouter, keeping the pairing capability
in the fragment (`/#/calibrate/:pairing`). The old `#calibrate=...` QR format is
normalized once for compatibility. Route selection happens before settings
load, so calibration never renders the dashboard first. Main routes retain
the Settings component and its unsaved draft; preview streaming follows the
active route.

From `crates/web/prysm-web/control-ui`:

- `npm run dev:calibration` builds and serves the actual UI at
  <http://127.0.0.1:5180/#/calibrate/local-demo>. Its in-memory API simulation
  has no TV, WLED, outbound requests, or persistent settings. The banner's
  **Restart demo** starts a new session. This is a single local demo session.
- `npx playwright install chromium webkit` installs browser test runtimes.
- `npm run test:browser` builds both frontends and runs the wizard in desktop Chromium, mobile Chromium,
  a 320px viewport, and mobile WebKit. Tests cover delayed loading, explicit
  physical-response confirmation, live updates, hold/release, static color
  selection, optional tones, reload/rejoin, failed save, successful save,
  profile preservation, routing history, and Stop. Screenshots and failure
  traces go to ignored `test-results/`. TV layout checks also ensure the remote
  footer stays within the safe area at 1080p and 720p.
- `npm test` runs the DOM/controller UI checks and the development proxy test.

The simulation proves UI behavior, not optical accuracy or TV decoding. The revised static-color flow passed the LG hardware smoke test: all six
colors maintained capture, decoded correctly, and drove the expected LED
output. The user confirmed the physical lights followed. See the capture
evidence linked above. Optical accuracy across picture modes is still unmeasured.
