# Single-tap capture cleanup — 2026-11-02

The user confirmed that only combined capture and video-only are needed. Version
0.1.23 removes the two-tap implementation and was installed on the project TV.
The user subsequently rejected compatibility aliases and requested deletion of
the old saved configuration before reinstalling; that reset was performed.

## Result and deletion audit

- VTCapture is the only native backend: mode 3 for Both (video and visible menus),
  mode 2 for Video. Both sources deliver packed NV12 without software blending.
- Deleted DILE and GM/HAL implementations and their three native headers. Removed
  their FFI layouts, mappings, fallback dispatch, and UI lifetime management.
- Deleted BGRA composition, RGB staging, alpha scans/blending, UI-only selection,
  and the obsolete UI/composer metrics. Removed the redundant backend `layer`
  and requested `composition_format` metric labels; updated the Grafana panel.
- Hardware H.264 validates NV12 directly. Removed RGB/NV16 conversion and its
  scratch buffer; `yuv` is now only a development dependency for qualification
  fixtures. DMA upload, synchronization and buffer ownership are retained.
- Retained software H.264 fallback, raw/LZ4, receiver RGB/NV16 compatibility,
  pixel/geometry validation, constructor poisoning, C++ exception containment,
  process watchdogs, capture lock, bounded retries and transport deadlines.
  These serve independent safety/protocol purposes and are not two-tap overhead.
- `Source` now has only Video and Both. `ui` is rejected, without an alias or
  migration. The TV app offers Video + TV menus and Video only.

Compared with the snapshot taken before this cleanup, code/config changes remove
2,374 lines and add 320 (net 2,054 fewer lines). Documentation and dashboard
changes are excluded from that figure. Existing dirty encoder optimizations
from earlier work are not counted as cleanup deletions.

## No-video behavior: measured limitation

A physical cold-start experiment launched Home and confirmed MAIN disconnected,
with zero dimensions and frame rate. Combined VTCapture failed to provide a
frame, returning `vtCapture_planeInfo failed: 13` four times. This disproved the
assumption that mode 3 could replace HAL on Home without active video.

Both sources now wait when readiness reports no video. Combined still includes
menus over active video; it does not provide standalone UI capture. HDMI1 and
the installed sender were restored after the experiment. This is an intentional
consequence of removing the UI-only fallback, not a claim that Home is captured.

## Validation

- Final hardware-feature host suite: 100 tests passed; two TV-only qualification
  tests ignored. Default/software-only suite also passed before the final alias
  deletion. Final source and UI serialization checks ran with hardware enabled.
- Strict host and ARM Clippy passed; ARM hardware build succeeded. The final
  0.1.23 package rebuilt worker and supervisor after the alias was removed.
- TV app: 17 tests passed. Package helper tests: 3 passed. Formatting and dashboard
  JSON validation passed. A pre-existing trailing blank line in Prometheus's
  target file was left untouched.
- Staged final capture implementation: 240 Combined and 60 Video frames at
  640×360, requested 30 FPS. All 300 decoded through the unchanged receiver;
  each connection's first picture decoded independently. No software fallback.
- Combined readiness unavailable/available paused and resumed encoding; counters
  stayed unchanged during the settled pause. Native reset resumed successfully.
  All Combined frames carried Blended provenance, with no UI/composer metrics.
- These are bounded checks, not long-duration stability, HDR calibration,
  cold-boot or broad TV model compatibility qualification.

## Clean reinstall

The old supervisor was stopped, `/var/lib/prysm-capture/config.json` deleted,
and absence checked before installing 0.1.23. Setup started with an empty receiver
and no config file. A fresh configuration was then saved through `setConfig`:

- receiver `192.168.18.18:19400`
- source Both; hardware H.264
- metrics and autostart enabled

The sender was started and reported enabled, connected, capturing, and no error.
All 21 installed package files matched their expected SHA-256 hashes.
Package SHA-256: `6f98b2cb980aa97363f0b867db62844d7f20aef0c4065add1723cf1f29af8a83`.

## Installed live performance

Over 30.026 seconds the installed worker encoded 900 Blended frames and the
computer processed 900: 29.97 FPS. Mean iteration time was 12.944 ms, leaving
20.389 ms (61.2%) of the 30 FPS budget; zero deadlines were skipped. Hardware
output wait remained dominant at 11.984 ms, versus 0.364 ms native acquisition.
This cleanup preserves the combined path's performance; the short sample does
not establish a statistically significant speedup over 0.1.22.

## Evidence

Local evidence under `work/webos-single-tap-cleanup/`:

- `before/` and `diff-summary.json`: per-file cleanup delta.
- `evidence/home-both-640x360.log`, `video-home.json`, and foreground snapshots:
  physical Home failure and HDMI restoration.
- `final/evidence/results.json`, wire streams and pause metrics; `final/replay.log`:
  final staged capture qualification and receiver decodes.
- `final-host-tests.log`, `final-default-tests.log`, `final-host-clippy.log`,
  `final-arm-build.log`, `app-tests.log`, `packaging-tests.log`, `package-build.log`.
- `deployment/evidence/config-reset.json` and `deployment.json`: clean-config
  reinstall, exact package files and service status.
- `deployment/live-summary.json`: post-install 30-second stream measurement.
