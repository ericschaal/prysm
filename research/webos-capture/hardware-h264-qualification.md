# Hardware H.264 qualification history

**Status:** historical measurements from 2026-11-01–02, across several implementations.
**Finding:** direct output removed a measured parser delay; persistent DMA mapping
reduced input overhead in short target-TV samples. No universal performance claim follows.
**Platform and limits:** project webOS 25 TV, stated resolutions and workloads.
Earlier RGB/NV16, UI-fallback and composition results describe removed paths.
**Current contract:** [hardware H.264](../../docs/developers/hardware-h264.md).

## Combined capture observation (0.1.22)

Version 0.1.22 supplies the native blended NV12 tap to this encoder for Both
capture. Visible UI no longer requires a separate HAL copy or CPU composition
when that tap is available. Readiness-driven UI fallback remains supported.
The deployed 640×360/30 fps sample averaged 12.9 ms per whole iteration; hardware
output wait remained dominant at 11.9 ms. See
[combined capture deployment](2026-11-02-encoder-latency/combined-deployment.md).

## Direct output qualification on 2026-11-02

The direct pipeline and current-frame header/content pairing passed the updated
ARM qualification on the TV. Every synthetic input, including the first, decoded
as its current picture. RGB/NV12/NV16, 640×280 at 30 FPS, repeated initialization,
geometry failure and an independently decodable software recovery were checked.
Default and hardware-enabled host tests, ARM cross-builds, and strict Clippy
checks passed. DMA ownership, normalization and fallback protections remain.

A staged production worker using both video and UI capture sent 300 hardware
frames at 640×280/30 FPS with no software fallback. Mean steady output interval
was 33.32 ms. Mean capture/encode/send iteration was 23.99 ms, including startup;
UI acquisition averaged 11.78 ms and encoding 10.42 ms. Metrics identify 299 video
frames and one startup UI frame. Steady worker CPU was 24.82% of one logical CPU,
including capture, encoding, metrics and TV-local TCP send; vendor debug logging
was enabled. This is a roughly ten-second sample of the TV's current content,
not a sustained workload or end-to-end LAN latency measurement.

Additional hardware/software worker sessions at 320×180/15 FPS and a software
640×280/30 FPS session passed. All 405 wire frames decoded with the unchanged
host receiver, including the first picture with fresh decoder state. Receiver
disconnect and SIGTERM exited cleanly. Temporary capture authorization and test
files were removed; installed settings were unchanged and the sender stayed
disabled during qualification. Subsequently, at the user's explicit request,
version 0.1.21 was installed and the sender enabled toward the current computer.
All 21 package files matched; the service reported connected/capturing without
error, and the host receiver decoded incoming H.264 frames. Other settings were
preserved. Deployment evidence (local evidence at `work/webos-encoder-latency/deployment/evidence/deployment.json`; not distributed).

The preceding controlled probe measured approximately 34 ms less frame latency
at 640×360/30 FPS after removing the parser; its encoder hardware time stayed
unchanged. See [integration evidence](2026-11-02-encoder-latency/integration-results.md)
and [controlled latency experiment](2026-11-02-encoder-latency/live-results.md).
Standby/resume, prolonged load, other firmware, 60 FPS and 4K remain unqualified.

## Persistent DMA mapping qualification on 2026-11-02

Version 0.1.20 retains each DMA mapping for its slot's lifetime, uploads native
NV12 directly, and allocates conversion scratch only when RGB/NV16 needs it.
Eight hardware phase histograms expose conversion, upload, synchronization start
and end, submission, output wait, collection and normalization. Grafana's
expanded **Hardware H.264** section below stream health separates preparation
and collection from output wait, so the smaller phases remain readable.
The metric catalog (`observability/CATALOG.md` in the tag `pre-1.0-consolidation`) defines their boundaries.

On the project TV, a baseline → updated → baseline experiment used the same
changing 640×360 inputs, 30 FPS and 2 Mbit/s. Each format had five warmup inputs
and forty measured outputs. Native NV12 encode wall time fell from
13.52–13.75 ms to 12.49 ms; CPU time during encode calls fell from 3.25–3.29 ms
to 2.01 ms. Measured native NV12 bitstreams were byte-identical. Updated RGB/NV16
wall times were 15.55/13.71 ms versus baseline 16.10–16.11/14.05–14.10 ms.
These are short synthetic samples, not whole-worker CPU or live latency results.

The updated NV12 phases measured 12.13 ms waiting for output, 0.12 ms uploading,
0.12 ms normalizing and 0.06 ms submitting; synchronization and output collection
were smaller. That leaves pipeline output wait as the dominant encoder duration.
The existing changing-input, color, format/geometry transition, metadata, fallback
and teardown qualification passed on the TV. Tests used the shared capture lock
and external watchdogs; saved settings and the enabled sender state were restored.
Evidence is under `work/webos-hardware-dma/`. That DMA-only revision retained
the extra frame of pipeline latency; the later direct-output change removes it.

## Rust bindings qualification on 2026-11-01

Both default and hardware-enabled ARM packages cross-built against the relocated
SDK. ELF dependency inspection confirmed that only the hardware worker links
GStreamer. Strict Clippy passed for the ARM hardware build and both host variants;
host tests, the TV app tests and package tests passed.

The staged ARM qualification test passed on the project TV under the shared
capture lock and an external watchdog. It exercised changing RGB, NV12 and NV16
inputs, geometry changes, delayed metadata, decoding, software recovery and
teardown. Separate staged supervisors returned `capabilities.hardwareH264` as
`true` and `false` for the corresponding builds through the TV's Luna bus.

Staged workers captured the TV UI and sent 35 frames each at 640×360 and 320×180,
15 FPS, using hardware and software encoding. Hardware never fell back to
software. All 140 recorded frames decoded through the unchanged host receiver;
the first picture of every connection also decoded with fresh decoder state.
Receiver disconnect and SIGTERM exited normally. This run used UI capture while
the TV was initially in Screen Saver; it did not qualify live HDMI capture or
sustained performance. The installed app and saved configuration were not changed.
Temporary service registrations and files were removed, and final installed
service status was idle and error-free.

Logs, staged artifacts and wire recordings are under
`work/webos-gstreamer-bindings/`. Replay the new recordings with:

```sh
PRYSM_HARDWARE_WIRE_DIR="$PWD/work/webos-gstreamer-bindings/runtime-results" \
  cargo test --locked -p prysm-webos-protocol --features h264 \
  real_worker_wire_sequences_decode_with_unchanged_receiver -- --ignored
```

## Historical qualification on 2026-11-01

The results below describe the earlier handwritten C binding implementation,
before conversion to the standard Rust bindings and default-off feature.
They do not qualify the current bindings. Including hardware then added
approximately 76 KiB (4.1%) to the stripped ARM worker; this is not a size
measurement of the current build.

On the project TV, bounded private probes held the shared capture lock. Temporary
Luna roles authorized staged test/worker executables and were removed afterwards.
The installed service and its capture setting were not changed.

- Original hardware SPS is rejected; all fifteen corrected fixture access units
  decode through the existing receiver. Non-SPS NALs remain byte-identical.
- Changing colored RGB, NV12 and NV16 inputs, non-macroblock-aligned heights,
  repeated creation, geometry changes, EOS and teardown were exercised on TV.
- Delayed header/content ownership and a returned geometry error followed by
  an independently decodable software IDR were checked through the sender encoder.
- Real 640×360 NV12 capture ran alongside encoding and receiver decoding for
  thirty pictures, followed by clean capture and encoder release.
- Actual hardware and software worker binaries each sent 35 wire frames at
  640×360 and 320×180, at 15 FPS. All 140 recorded frames decoded through the
  unchanged receiver, including each connection's first independently decodable
  picture. Receiver disconnect and SIGTERM exited normally.

Measured worker CPU includes capture, encode, instrumentation and TV-local TCP
send, without a decoder in the worker. Samples cover thirty steady frame
intervals (about two seconds), with one logical CPU equal to 100%:

| Resolution | Hardware CPU | Software CPU | Hardware first frame | Software first frame |
| --- | ---: | ---: | ---: | ---: |
| 640×360 | 13.5% | 9.5% | 862 ms | 227 ms |
| 320×180 | 11.5% | 3.5% | 314 ms | 222 ms |

The first hardware run includes cold GStreamer discovery. Mean instrumented
capture/encode/send iteration times, including startup, were 33.7/14.6 ms for
hardware and 10.3/6.4 ms for software at the respective resolutions. Hardware's
additional one-frame queue delay is separate from those iteration timers.
Observed worker exit times after disconnect/SIGTERM were 165–215 ms for hardware
and 114–165 ms for software. These are short samples of the TV's current content,
not a sustained motion benchmark or LAN end-to-end latency measurement.

Hardware remains an explicit compression choice: the measured implementation did not improve CPU or
latency at the measured resolutions. Standby transitions, prolonged stress,
other firmware and other resolutions/frame rates remain unqualified. Existing
supervisor tests exercise power-stop and stalled operation recovery; no TV power
state was changed for qualification.

The local evidence and bounded TV-local worker probe are under
`work/webos-hardware-integration/`. This research directory is not a runtime
or test dependency. Recheck TV health and arrange an idle window before running
hardware tests; retain the capture lock and an external process watchdog.

Host checks:

```sh
cargo test --locked -p prysm-webos-capture -p prysm-webos-service -p prysm-webos-protocol -p prysm-capture
cargo clippy --locked -p prysm-webos-capture -p prysm-webos-service -p prysm-webos-protocol --all-targets -- -D warnings
```

On Linux with GStreamer development libraries installed, repeat these checks
with `--features prysm-webos-capture/hardware-h264,prysm-webos-service/hardware-h264`
to check the native binding path. macOS feature checks use the unavailable-backend
stub and do not verify Linux GStreamer integration.

The ignored ARM test
`encoder::hardware::qualification::hardware_capture_qualification` takes the
capture lock itself and requires `--features hardware-h264` when building the
capture test binary. `PRYSM_QUALIFY_REAL_CAPTURE=1` additionally requires a Luna
role for its executable and an active video source. Run under a bounded external
watchdog, never alongside the installed capturer. The host receiver replay is:

```sh
PRYSM_HARDWARE_WIRE_DIR="$PWD/work/webos-hardware-integration/wire-results" \
  cargo test --locked -p prysm-webos-protocol --features h264 \
  real_worker_wire_sequences_decode_with_unchanged_receiver -- --ignored
```
