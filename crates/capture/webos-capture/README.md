# Prysm webOS capture

Rust CLI for capturing video, TV UI, or both on a **rooted LG TV**, and streaming
frames directly to Prysm. Capture scheduling, pixel processing, recovery, CLI,
and transport are Rust. The four native backends load LG's system libraries
directly. There is no Hyperion daemon, C sender, CMake build, or backend plugin
ABI. The receiver implements one bounded raw/LZ4/H.264 protocol.

The native adapters derive from MIT-licensed [hyperion-webos](https://github.com/webosbrew/hyperion-webos)
revision `00c932f092084b234b3f619b060a98fa134c12bd`; retain the supplied license
notices. Project-owned C/C++ is limited to vendor API headers, a typed exception trampoline
for LG's C video API, and an ABI/exception test fixture. LG can throw C++
exceptions through its C API, so a C++ boundary is necessary to contain them.
No type-erased calls, numeric operation dispatcher, or function-pointer casts
are used. H.264 uses the `openh264` crate with its bundled native codec. All project-owned unsafe Rust is confined to `src/native`; the rest of the sender
denies unsafe code at compile time.

The Luna setup and native video startup were previously verified on an LG
43QNED87A6D.BEUFLJP running webOS 25 (10.3.2-34), with UI disabled. The Rust sender
has also completed a short 640×240/30 fps test with both layers and LZ4 on that TV.
Root access does not guarantee compatibility with other drivers or correct colors.

## Build for webOS

Install and relocate the [buildroot-nc4 webOS SDK](https://github.com/openlgtv/buildroot-nc4),
then from the repository root:

```sh
rustup target add armv7-unknown-linux-gnueabi
WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
  ./packaging/webos/build-ipk.sh
```

The shared package build compiles the sender and supervisor with the SDK C++ compiler and linker,
an explicit sysroot, Cortex-A9 code generation, and the **soft-float ABI**
(`gnueabi`, not `gnueabihf`). The `webos` Cargo profile enables optimization, LTO,
one codegen unit, and stripped symbols. The executable uses `/lib/ld-linux.so.3`.
Rust normally targets glibc 2.17+, while this SDK supplies glibc 2.12; the linker
explicitly includes the SDK's `libglibc_polyfills.a` for `getauxval`. This is a
webOS compatibility build; runtime behavior still needs hardware validation.

Binaries are in `target/armv7-unknown-linux-gnueabi/webos/`; the IPK and license
notices are assembled by the [TV package build](../../../packaging/webos/README.md).
There is no separate distribution staging tree or per-crate build script.
LG's capture libraries, `libstdc++.so.6`, and `libgcc_s.so.1` come from the TV.

## Usage

Start Prysm with this input and enable Screen sync to open the receiver:

```toml
[input]
type = "webos"
listen = "0.0.0.0:19400"
```

The sender takes one receiver IP address and port (IPv4 or bracketed IPv6):

```sh
# Lowest TV CPU cost: stream the video planes without color conversion.
prysm-webos-capture 192.168.1.10:19400 --source video

# Composite the TV's UI onto video; this is the default capture mode.
prysm-webos-capture 192.168.1.10:19400 --source both --compression lz4

# Reduce composed payload size with NV12 before LZ4.
prysm-webos-capture 192.168.1.10:19400 --source both \
  --composition-format nv12 --compression lz4

# UI capture works independently, without opening a video driver.
prysm-webos-capture 192.168.1.10:19400 --source ui --ui-backend halgal

prysm-webos-capture --help
```

The receiver owns capture dimensions and frame rate. Set
`input.capture_resolution = [width, height]` and `input.refresh_rate_hz` in Prysm's
configuration. Dimensions must be even and within 2–1920 × 2–1080; FPS must be
1–60. The sender negotiates both before opening native capture and configuring
H.264, and recreates video capture and UI surfaces when resolution changes.
There are no `--width`, `--height`, or `--fps` options on the sender. Other defaults
remain both layers, automatic backend selection, and no compression. VTCapture
receives the native FPS; DILE uses its frame divisor plus the sender loop's cadence
to avoid capturing or sending excess frames.

| Option | Choices / purpose |
| --- | --- |
| `--source` | `video`, `ui`, `both` |
| `--compression` | `none`, `lz4` (lossless, raw fallback), `h264` (lossy, low latency) |
| `--h264-bitrate` | H.264 target bits/sec; default `2000000` |
| `--composition-format` | `rgb` (default), `nv12`; applies after UI composition, including UI-only fallback |
| `--video-backend` | `auto`, `vtcapture`, `dile` |
| `--ui-backend` | `auto`, `gm`, `halgal` |
| `--capture-point` | `display`, `scaler-input`, `scaler-output`, `blended`, `osd`, `histogram` |
| `--force-capture` | Use an installed `/dev/forcecapture` kernel module when vtcapture is not ready |
| `--dile-create-ex` | Use the alternate DILE constructor on TVs that need it |
| `--no-freeze` | Skip DILE buffer freezing; may produce tearing |

Hardware settings are named and typed; there is no numeric quirk mask. Use only
settings verified for your TV. DILE supports `display` and `scaler-output` and
rejects dimensions below its reported minimum. Backend flags are relevant only
when their layer is selected. With both layers selected, unavailable UI falls
back to video. Explicit UI-only capture reports UI initialization failures.

## Low-latency H.264

Set `input.capture_resolution = [640, 240]` on the receiver, rebuild both Prysm
and the TV distribution, then run on the TV:

```sh
./prysm-webos-capture 192.168.18.18:19400 --source both \
  --compression h264 --h264-bitrate 2000000 \
  --metrics-listen 0.0.0.0:9101
```

OpenH264 runs in software with ARM NEON, baseline IP frames, one encoder thread,
low complexity, and rate-control frame skipping. Periodic keyframes are disabled:
TCP delivers every reference frame, and each new connection starts with an IDR.
Scene-change detection remains enabled. This avoids spending the bandwidth budget
on a forced keyframe every second; there is no seeking or midstream joining.
Bitrate is a target, not a strict per-frame or instantaneous limit.
When the bitrate budget is exhausted, the encoder can skip an input without
sending a packet or resetting the connection. Metrics report this as
FrameEncoder/EncodeH264/SkippedRateControl. The receiver retains its last frame
until the next encoded frame arrives.
The default RGB composition converts directly to limited-range BT.709 I420 with
balanced conversion precision; the decoder uses the same range and matrix.
Leave `--composition-format` at its default to avoid an extra NV12
conversion. Native NV12/NV16 video converts to I420 without an RGB round trip.
The tested TV's default display capture tap produces limited-range BT.709,
confirmed from the live scaler registers and the exact firmware's conversion
table. Native decoding, composition, H.264 VUI, and the receiver use that same
matrix and range. See the [colorimetry investigation](../../../research/webos-capture/2026-11-01-colorimetry/README.md).
H.264 carries chroma subsampling and lossy quantization, so compare LED colors
and dark scenes as well as bitrate, frame rate, encoder time, receiver DecodeH264
time and both machines' CPU use. Hardware encoding is not used.

Each TCP connection owns fresh codec state and begins with SPS/PPS plus an IDR.
The receiver decodes every access unit before publishing the latest frame; it
never drops reference packets to catch up. A write failure closes the connection,
discards encoder state, and exits. The TV service starts a fresh worker. Slow networking
can still cause TCP head-of-line delay; there is no frame queue in the sender.
The new binary has been cross-built with the webOS SDK; H.264 performance and
thermal behavior still need validation on the TV.

For a constrained link, set `--h264-bitrate` below its sustained available payload
bandwidth, leaving room for TCP overhead and keyframe bursts. The default is
2 Mbit/s; the TV app currently uses that default. Reduce the receiver's capture
resolution before forcing severe quantization or persistent frame skipping.
Keep low complexity until encoder timing on the TV shows headroom for more work.

## TV setup

Use the [TV app package](../../../packaging/webos/README.md) to install the worker
and Rust supervisor and register their separate executable Luna roles. Root alone
does not grant permission to register native capture clients. Standalone launches
must use the executable path named by the installed capture role. Old system
manifest overlay templates are no longer distributed.

Ctrl+C and SIGTERM request shutdown through a signal-waiting thread; native capture
threads inherit blocked shutdown signals to avoid interrupting vendor calls.
Native video release has a reproduced lost-wakeup hang on the tested TV; the
supervisor retains a shutdown deadline and process-level recovery. See the
[cleanup diagnosis](../../../research/webos-capture/2026-11-01-lifecycle/cleanup-diagnosis.md)
for evidence. Protected video may be unavailable; HDR is not automatically tone-mapped.

## Performance and protocol

Video-only capture packs NV12/NV16 directly from the driver. Color conversion
runs on the Prysm host. NV12 uses half the bandwidth of RGB24: at 640×360/30 fps,
raw pixels need 83 Mbit/s instead of 166 Mbit/s, before network overhead.
320×180 NV12 needs about 21 Mbit/s. UI blending converts video directly to RGB
and blends premultiplied BGRA in place, without an ARGB intermediate frame.
`--composition-format nv12` converts the composed RGB image to packed NV12
(limited-range BT.709, matching the receiver) before compression. It reuses the
RGB and NV12 allocations, requires no receiver or protocol changes, and halves
the raw payload. At 640×240/30 fps this is 55.3 Mbit/s instead of 110.6 Mbit/s,
before compression and network overhead. UI-only output also uses the selected
composition format; native video-only frames and UI-failure fallback pass through
unchanged. RGB remains the default for controlled comparisons.

NV12 subsamples chroma over 2×2 pixels, so this conversion is lossy even though
LZ4 is lossless. Colored UI edges can soften and RGB/YUV rounding can change
colors slightly. Compare decoded colors and black-bar behavior on your TV.
The extra conversion is measured separately as `Composer / ConvertToNv12`;
actual wire frame counters carry the NV12 header format.

The Rust `yuv` dependency uses a scalar ARMv7 conversion path; measure UI-enabled
CPU usage on the TV before increasing resolution.

Buffers are reused. Checked slice copies omit native stride padding, avoid
zero-filling, and release frozen native frames before network writes. Failed
acquisition does not send stale pixels. Missed ticks are skipped. Video open and
capture errors retry after one second, including at startup; UI frames continue
while video is unavailable. Driver errors remain logged until capture succeeds
or the sender is stopped. Vtcapture status 2 means initialization is still pending;
the sender polls the same driver for up to two seconds before restarting. Its
release function deletes the driver without returning a status; only exceptions
are treated as release failures. Every successful acquisition is copied, even when
the driver reuses a buffer address. Without a frame sequence or timestamp, a repeated
address cannot distinguish a fresh frame from a stalled driver, so it does not
trigger a restart.

`--compression lz4` independently compresses each frame using a pure Rust block
codec and reusable storage. Expanded frames are sent raw. Compression is off by
default because noise/film grain can cost CPU without saving bandwidth. Run the
benchmark below on representative hardware; synthetic flat/gradient/noise data
does not establish TV performance. On the development ARM64 Mac, 640×360 NV12
synthetic encoding took roughly 9–11 µs per frame; these are host measurements.

There is no frame queue. Connect and complete-frame writes each have a 100 ms
deadline; partial writes close the connection and retry after one second. During
network backoff, capture and composition are paused.
Safe `nix` polling preserves one deadline across every partial write. TCP buffers can
still retain frames briefly, so this is not a hard end-to-end latency guarantee.

Each TCP connection begins with bounded, length-prefixed Serde JSON control
messages (four-byte unsigned big-endian length, 1–128 bytes). The sender sends
`{"type":"hello","version":VERSION}`. The active `VERSION` comes from
[`wire-version.txt`](../prysm-webos-protocol/wire-version.txt); Rust, tests, and the
TV smoke receiver all read that file. Bump it when changing the wire contract.
Version 3 defines YUV as limited-range BT.709,
replacing version 2's BT.601 assumption. Rebuild/update both the TV worker and
Prysm receiver together: mixed versions reject the handshake instead of silently
changing hues. RGB remains full-range RGB24. Prysm responds with either
`{"type":"idle"}` when capture is off, or
`{"type":"capture_settings","refresh_rate_hz":30,"capture_resolution":[640,360]}`.
After an idle response, the sender waits for capture settings without opening
video/UI devices. The initial handshake and each partial control message have a
one-second deadline; the idle wait is interruptible and has no time limit.
Unknown versions, message types/fields, invalid rates/dimensions, and oversized
messages are rejected.

Prysm listens while running with webOS input, independently of pipeline state.
Starting the pipeline sends settings to an idle TV. Stopping it closes that
capture connection; the worker exits and the TV service launches a fresh worker
that reconnects idle. Initial connection failure also exits; only the service
retries. This process boundary reclaims native LG state retained during UI
teardown. Changing input rate or resolution starts a new capture session with
fresh native resources and H.264 encoder. Update the host and TV package together
to use idle connections. Standalone invocation handles exactly one connection.

After the handshake, frames retain their 24-byte binary header and payload:

| Offset | Bytes | Value |
| --- | --- | --- |
| 0 | 4 | ASCII `PRY` followed by the version digit from `wire-version.txt` |
| 4 | 4 | Width, unsigned big-endian |
| 8 | 4 | Height, unsigned big-endian |
| 12 | 4 | Format: `0` RGB24, `1` NV12, `2` NV16 |
| 16 | 4 | Codec: `0` raw, `1` LZ4 block without a size prefix, `2` H.264 Annex B access unit |
| 20 | 4 | Payload byte length, unsigned big-endian |
| 24 | length | Tightly packed pixels, their LZ4 block, or one complete H.264 access unit |

NV12 is Y followed by interleaved UV at half height; NV16 uses full-height UV.
For H.264, format `0` describes the decoded RGB24 output, not the internal I420
encoder surface. H.264 payloads are capped at 8 MiB; SPS coded/cropped geometry
and baseline profile are validated before native decoding. Malformed or delayed
access units disconnect, rather than allowing reference-state corruption.
Native padding is omitted. Maximum dimensions are 1920×1080. NV12 requires
even width/height and NV16 requires even width. Allocation bounds and decoded
length come from validated dimensions, never a compressed size prefix. Unknown
formats/codecs, invalid lengths, corrupt LZ4, truncated frames, and stalled peers
disconnect. Receiver scratch buffers are reused; only complete latest frames are
published. Both sender and receiver must use this protocol; no legacy protocol
or command-line aliases remain.

There is no authentication or encryption. Bind on a trusted LAN interface and
restrict port 19400 to the TV. One TV connection is handled at a time.

## Verification

```sh
cargo test --locked -p prysm-webos-capture -p prysm-webos-protocol -p prysm-capture
cargo run --release -p prysm-webos-protocol --example compression
```

Tests cover typed CLI validation/help, native ABI layouts, C++ exception
containment, reused video buffers, pending initialization without premature restart,
startup retries with UI output, DILE unfreezing after acquisition failures,
HAL surface cleanup after mapping failure, skipped UI
work during network backoff, padded planes, color conversion/blending,
raw/LZ4 boundaries, malformed input, retry/backoff, complete-frame deadlines,
and receiver cancellation. CI also runs sender/protocol tests as ARM32 soft-float
executables under QEMU. Cross-build separately with the actual SDK script.
These checks do not load LG drivers or establish hardware performance or colors.

## Centralized metrics

Add `--metrics-listen 0.0.0.0:9101` to expose opt-in Prometheus metrics.
See [centralized monitoring](../../../observability/README.md) for the computer
Docker Compose stack, dashboard, node setup and measurement catalog.

### Video constructor failures

Before calling `vtCapture_create`, the worker registers and unregisters unique
names under both capture/resource-manager Luna prefixes. Errors are freed on all
paths; an unregister failure aborts rather than leaving live native registration
behind. The ABI is generated from the TV SDK by the service binding generator.
This precheck prevents the measured name-registration denial leak; it does not
guarantee later outbound permissions or native initialization success.

A constructor failure poisons process-global vendor state. It now ends that worker
with a typed failure event, without automatic backend fallback or further capture
attempts in the same process. The library stays loaded until process exit.
Under the installed supervisor, Luna MAIN-sink readiness controls video startup
over a private stdin pipe. Without video, UI capture and the receiver connection
continue while video-only capture waits. Any HDMI input or built-in video source
can become ready; there is no HDMI1 filter. A source/context transition resets
only the video client. The supervisor bounds that reset with a five-second
watchdog and recycles the worker on timeout.

Ordinary missing-backend errors may still fall back. Standalone or unknown-readiness
capture retries not-ready preparation for up to ten seconds. A native client that
stays unready emits failure before driver teardown, which can block. The supervisor
retains its five-second kill deadline and failure backoff. Known video absence
does not cause periodic worker restarts. Stop cancels retries.

Status 1 from LG init/preprocess can leave the session finalized. Version 0.1.9
reinitializes that session on the same driver before retrying preprocessing;
repeating preprocessing on a closed device cannot recover. Retries are paced at
one second and bounded by the existing ten-second deadline. The supervisor also
watches native startup/reinitialization, preserving worker recovery on a hang.

The installed 0.1.9 wake test reproduced the initial preprocessing failure and
recovered by session reinitialization in 1.09 seconds, retaining the wake worker
and receiver connection. The user confirmed visible video within about two
seconds of the TV picture. Full OS cold boot and other physical HDMI ports remain
unverified; this test used suspend/resume with Freebox on HDMI1.
