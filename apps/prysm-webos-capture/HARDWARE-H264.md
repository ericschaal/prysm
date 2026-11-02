# Experimental webOS hardware H.264

Current source uses only native NV12 capture: combined or video-only. Legacy
UI fallback, blending and RGB/NV16 hardware conversion have been removed. Both
sources wait when video is unavailable. Earlier release measurements below
describe the implementation at the time. See the
[single-tap cleanup audit](../../research/webos-capture/2026-11-02-encoder-latency/single-tap-cleanup.md).

Version 0.1.22 supplies the native blended NV12 tap to this encoder for Both
capture. Visible UI no longer requires a separate HAL copy or CPU composition
when that tap is available. Readiness-driven UI fallback remains supported.
The deployed 640×360/30 fps sample averaged 12.9 ms per whole iteration; hardware
output wait remained dominant at 11.9 ms. See
[combined capture deployment](../../research/webos-capture/2026-11-02-encoder-latency/combined-deployment.md).

Hardware H.264 is excluded from default builds. Build with
`./packaging/webos/build-ipk.sh --hardware-h264` to enable the `hardware-h264`
Cargo feature in both the worker and supervisor. The TV app always offers
**H.264 (software)** (`h264-software`, OpenH264), and adds **H.264 (hardware)**
(`h264-hardware`) when status reports `capabilities.hardwareH264: true`.
This field reports build support, not runtime encoder availability. Default
supervisor builds reject hardware configuration; the old `h264` setting is
not accepted.

Build the accompanying supervisor from this revision too: hardware operation
watchdog events extend the private worker IPC. Hardware selection is preserved
through saved service configuration and the worker startup contract; both choices
use the same H.264 wire codec and receiver.

On Linux the backend links to the TV's GStreamer libraries through the standard
`gstreamer`, `gstreamer-app`, `gstreamer-video` and `gstreamer-allocators` Rust
bindings. The libraries and vendor plugins are not bundled. A hardware-enabled
worker requires those shared libraries even when software compression is selected;
missing libraries prevent startup before software fallback can run. The checked
build SDK supplies GStreamer 1.20.4 and GLib 2.72.3. No newer API version features
are enabled. Non-Linux builds use an unavailable-backend stub for host tests.

The pipeline connects `rvenc` directly to `appsink` with two persistent, CPU-mapped
192-byte-pitch NV12 DMA-BUFs. Native NV12 rows upload directly into the selected
slot without conversion scratch or an intermediate image copy. Hardware input
is validated as native limited-range BT.709 NV12. DMA padding is initialized once per slot; direct NV12
uploads preserve it. Cache synchronization remains paired around every CPU
write. A slot is reused only after GStreamer releases its input reference. Per-frame allocate/free caused reproduced
picture corruption; persistent allocations fixed the changing-input regression.
The vendor's internal cause has not been established.

The TV's `h264parse` stage waits for the next input even after `rvenc` has emitted
a complete picture. Removing it lets every encode call return its own picture,
including the first input. There is one submitted picture awaiting output;
header/content metadata belongs to that same input, without a pending record.
The vendor advertises AVC statically but emits Annex B, so the direct graph has
no forced output caps. Actual bitstreams still pass the normalizer's validation:
B/SP/SI slices and multiple pictures per access unit are rejected. Session
termination drains EOS; references never continue into a new connection.

SPS correction changes only cropping and video-signal metadata. Cropping is
derived from coded versus visible dimensions; other SPS/VUI data is preserved.
The receiver's dimension, sequence, length and limited BT.709 checks are unchanged.
The stream must begin with SPS/PPS/IDR. Initialization or first-frame failure
falls back to software before transmitting a hardware reference. Later hardware
failure closes the socket, releases resources under watchdog protection and
negotiates a new software session with fresh codec state.

GStreamer pipelines, buffers, samples and references use typed Rust bindings.
Kernel DMA allocation, descriptor transfer and mappings remain private to
`encoder/gst.rs`. Callers supply Rust slices and validated dimensions; they cannot
supply native pointers or capacities. There are no Rust callbacks or manually
implemented Send/Sync traits. Ownership and timed pulling
follow the public [appsrc](https://gstreamer.freedesktop.org/documentation/applib/gstappsrc.html),
[appsink](https://gstreamer.freedesktop.org/documentation/applib/gstappsink.html),
and [DMA-BUF allocator](https://gstreamer.freedesktop.org/documentation/allocators/gstdmabuf.html)
contracts. No private vendor ABI or FFmpeg subprocess is used.

Output waits have a one-second deadline and EOS has a 500 ms wait. Native calls
including creation, fallback teardown and final NULL transition are covered by
the supervisor's five-second encoder watchdog and subsequent kill deadline;
a blocking vendor call is not assumed interruptible. A watchdog-killed process
cannot perform in-process software fallback; a subsequent worker with hardware compression selected may attempt hardware again.

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
preserved. [Deployment evidence](../../work/webos-encoder-latency/deployment/evidence/deployment.json).

The preceding controlled probe measured approximately 34 ms less frame latency
at 640×360/30 FPS after removing the parser; its encoder hardware time stayed
unchanged. See [integration evidence](../../research/webos-capture/2026-11-02-encoder-latency/integration-results.md)
and [controlled latency experiment](../../research/webos-capture/2026-11-02-encoder-latency/live-results.md).
Standby/resume, prolonged load, other firmware, 60 FPS and 4K remain unqualified.

## Persistent DMA mapping qualification on 2026-11-02

Version 0.1.20 retains each DMA mapping for its slot's lifetime, uploads native
NV12 directly, and allocates conversion scratch only when RGB/NV16 needs it.
Eight hardware phase histograms expose conversion, upload, synchronization start
and end, submission, output wait, collection and normalization. Grafana's
expanded **Hardware H.264** section below stream health separates preparation
and collection from output wait, so the smaller phases remain readable.
The [metric catalog](../../observability/CATALOG.md) defines their boundaries.

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
