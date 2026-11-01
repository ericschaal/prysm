# Experimental webOS hardware H.264

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

The pipeline uses `rvenc` and
`h264parse`, and copies validated RGB/NV12/NV16 capture input into two persistent
192-byte-pitch NV12 DMA-BUFs. RGB conversion and NV16 vertical chroma averaging
produce limited-range BT.709. Padding is initialized. A slot is reused only after
GStreamer releases its input reference. Per-frame allocate/free caused reproduced
picture corruption; persistent allocations fixed the changing-input regression.
The vendor's internal cause has not been established.

GStreamer needs the next input to delimit the previous access unit on this
firmware. There is exactly one pending input header/content record and at most
two submitted frames. This adds one frame interval (66.7 ms at 15 FPS). Output
pairing uses submission order, not the vendor's repeated PTS. B/SP/SI slices and
multiple pictures per access unit are rejected. Session termination drains EOS
and discards the final unsent picture; references never continue into a new
connection.

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
