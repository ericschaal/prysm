# Combined capture integration and deployment, 2026-11-02

Version **0.1.22** is installed on the project TV and sending to the existing
computer receiver. All **21 installed payload files** match the package.
The receiver, Both source, hardware H.264, metrics and autostart settings were
preserved byte-for-byte. Sender status is enabled, connected, capturing and
error-free.

IPK SHA-256:
`94d18d4963cf60396365e3501410bc64159d6641bd9e2390d5bd41cfd90daaae`.

## Production change

- Both selects VTCapture's blended tap (3). Its NV12 frames bypass separate UI
  capture and CPU composition, and carry `Content::Blended` provenance.
- Video-only retains the display tap (2); UI-only retains GM/HAL capture.
- A nonfatal blended-tap opening error tries the existing display/DILE path.
  Poisoned native construction remains fatal; it is not retried in-process.
- Missing or pending video still allows UI frames on the same connection.
  Both defers UI allocation until fallback is needed, retaining native resource
  thread ownership, size validation, cleanup and watchdog behavior.
- `VtcaptureBlended` identifies the active video backend. The previous native
  UI-call/copy timing split is included in the installed build. CPU-composition
  counts remain distinct from hardware-blended output counts.

No new user-facing setting, protocol field, dependency or permanent debug
override was added. The existing separate-layer implementation remains the
fallback. Combined mode cannot automatically detect a driver that silently
accepts the tap but omits some UI; compatibility beyond the qualified TV is
not established.

## Qualification

The staged worker was byte-identical to the packaged worker. With hardware
H.264, the test exercised:

| Case | Frames | Result |
| --- | ---: | --- |
| Both, 640×360, initial acquisition | 90 | 89 combined, 1 startup UI fallback |
| Supervisor marks video unavailable | 30 | 30 UI-only fallback frames |
| Video availability returns | 60 | 58 combined, 2 transition UI frames |
| Supervisor resets capture | 60 | 59 combined, 1 transition UI frame |
| Video-only, 640×360 | 30 | Video frames, no combined provenance |
| UI-only, 320×180 | 30 | UI frames, no combined provenance |

All **300 wire frames** decoded with the unchanged receiver; each connection's
first frame also decoded independently. Decoded screenshots verify the test
toast in initial combined output, UI-only fallback and recovered combined
output. No software-encoder fallback occurred. Disconnect teardown completed,
temporary Luna roles were removed and the old sender was restored before
package installation.

The unavailable/reset cases use the real worker's supervisor control pipe;
they do not simulate physical power loss or an HDMI hotplug.

Host checks passed: 4 metrics tests, 53 capture unit tests, 1 capture integration
test, 42 service tests, 3 packaging tests and 17 app/protocol tests. The existing
hardware-only test is ignored on the host. Strict host and ARM Clippy passed.
Regression coverage checks native combined-frame provenance, no double UI
capture, UI fallback for pending/unavailable/failed video, recovery and deferred
UI resize configuration.

## Installed live measurement

After deployment, two metric snapshots approximately 30.07 seconds apart showed:

| Measurement | Result |
| --- | ---: |
| Combined H.264 frames encoded | 901 |
| Receiver processed frames | 900 |
| Approximate encoding rate | 29.97 fps |
| Whole TV iteration, mean | **12.872 ms** |
| Hardware output wait, mean | 11.933 ms |
| Encoder preparation, mean | 12.289 ms |
| Native video acquisition, mean | 0.344 ms |
| Socket write, mean | 0.134 ms |
| Separate UI captures / CPU blends | **0 / 0** |
| Skipped deadlines | **0** |
| Average 30 fps budget remaining | **20.462 ms / 61.4%** |

Snapshots are not synchronized across metrics and host state; the one-frame
count difference is not a measured loss. There were no observed counter resets.
This measures local processing headroom, not glass-to-glass latency or 60 fps
support. Hardware wait remains the dominant stage.

## Evidence and remaining limits

- [Sanitized deployment verification](combined-integration/deployment.json)
- [Qualification source/frame counts](combined-integration/results.json)
- [Unchanged-decoder replay](combined-integration/replay.txt)
- [Installed live timing/counts](combined-integration/live-summary.json)
- [Bounded qualification harness](combined-integration/wire-probe.py)

Complete raw evidence remains in ignored `work/webos-combined-integration/`
and `work/webos-combined-deployment/`. Captured screen content and receiver
addresses are not copied into the research report.

HDR/color calibration of the blended tap, actual power/input transitions,
source-frame freshness and long-duration stability remain unqualified. UI
fallback and controlled capture reset were tested as described above.
