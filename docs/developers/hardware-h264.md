# Experimental webOS hardware H.264

Current source uses only native NV12 capture: combined or video-only. Legacy
UI fallback, blending and RGB/NV16 hardware conversion have been removed. Both
sources wait when video is unavailable. Earlier release measurements in the qualification archive
describe the implementation at the time. See the
[single-tap cleanup audit](../../research/webos-capture/2026-11-02-encoder-latency/single-tap-cleanup.md).

Hardware H.264 is excluded from default builds. Build with
`cargo xtask package webos --hardware-h264` to enable the `hardware-h264`
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

## Qualification and checks

See [historical qualification and measurement limits](../../research/webos-capture/hardware-h264-qualification.md).

Host checks:

```sh
cargo test --locked -p prysm-webos-capture -p prysm-webos-service -p prysm-webos-protocol -p prysm-capture
cargo clippy --locked -p prysm-webos-capture -p prysm-webos-service -p prysm-webos-protocol --all-targets -- -D warnings
```

On Linux with GStreamer development libraries installed, repeat these checks
with `--features prysm-webos-capture/hardware-h264,prysm-webos-service/hardware-h264`
to check the native binding path. macOS feature checks use the unavailable-backend
stub and do not verify Linux GStreamer integration.
