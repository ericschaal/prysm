# Prysm webOS capture worker

Developer contract. For installation and operation, use [TV setup](../../docs/users/webos.md).

Native Rust worker for `prysm-webos-service` on a rooted LG TV. Configure the
receiver, capture source, and compression through the packaged **Prysm Capture**
TV app. The worker has no command-line interface or standalone mode.
When **Prometheus metrics** is checked and saved in the TV app, the running worker
exposes metrics at `http://<tv-ip>:9101/metrics`. The setting defaults off.
This endpoint has no authentication; restrict access to your trusted monitoring network.
Exporter startup failure is logged without interrupting capture.

The native adapters derive from MIT-licensed
[hyperion-webos](https://github.com/webosbrew/hyperion-webos), revision
`00c932f092084b234b3f619b060a98fa134c12bd`; retain the supplied license notices.
Project-owned C++ contains vendor exceptions before they can cross the Rust FFI
boundary. H.264 defaults to OpenH264's bundled native codec. Unsafe Rust is confined to
the LG adapters, kernel DMA allocation/mapping in the optional hardware encoder,
and inherited event descriptors. GStreamer ownership uses the standard Rust bindings.

The tested hardware is LG 43QNED87A6D.BEUFLJP, webOS 25 (10.3.2-34). Native YUV
uses the verified limited-range BT.709 display capture path, independently of the
scaled output resolution. Captured HDR is not automatically tone-mapped. Root
access alone does not establish compatibility or calibrated colors on another TV.

## Build for webOS

Install and relocate the [buildroot-nc4 webOS SDK](https://github.com/openlgtv/buildroot-nc4),
then from the repository root:

```sh
rustup target add armv7-unknown-linux-gnueabi
WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
  ./packaging/webos/build-ipk.sh
```

Hardware H.264 is excluded by default. Add `--hardware-h264` to the package
command to enable it in both binaries. On Linux this links the worker to the
TV's GStreamer core, app, video, allocators and GLib libraries; these libraries
are not bundled. They must be present even if software compression is selected.
The script uses the SDK's target pkg-config metadata. The checked SDK supplies
GStreamer 1.20.4 and GLib 2.72.3; newer API version features are not enabled.

The shared package build compiles the sender and supervisor with the SDK C++ compiler and linker,
an explicit sysroot, Cortex-A9 code generation, and the **soft-float ABI**
(`gnueabi`, not `gnueabihf`). The `webos` Cargo profile enables optimization, LTO,
one codegen unit, and stripped symbols. The executable uses `/lib/ld-linux.so.3`.
Rust normally targets glibc 2.17+, while this SDK supplies glibc 2.12; the linker
explicitly includes the SDK's `libglibc_polyfills.a` for `getauxval`. This is a
webOS compatibility build; each target firmware and hardware change needs its own validation.

Binaries are in `target/armv7-unknown-linux-gnueabi/webos/`; the IPK and license
notices are assembled by the [TV package build](../../docs/developers/packaging.md#webos).
There is no separate distribution staging tree or per-crate build script.
LG's capture libraries, `libstdc++.so.6`, and `libgcc_s.so.1` come from the TV.

## Worker startup and ownership

The service starts the worker without arguments and supplies:

- `PRYSM_WORKER_CONFIG`: bounded JSON containing a validated receiver address,
  `source` (`video`, `both`), `compression` (`none`, `lz4`, `h264-software`, `h264-hardware`), and
  `metrics` (boolean, defaults off).
- `PRYSM_EVENTS_FD`: an owned inherited pipe for typed lifecycle events.
- stdin: the required private readiness/reset control pipe.

The shared types and byte encoding live in `prysm-webos-protocol`. Missing or
malformed startup configuration, missing event descriptors, and command-line
arguments fail before native capture begins. The worker must run as root at the
executable path authorized by the installed capture role. Install and register it
using the [TV package](../../docs/users/webos.md).

Each worker owns exactly one receiver connection. It connects and negotiates
before opening native resources. An idle receiver opens no native capture devices.
Prysm owns the requested frame rate and resolution; stopping capture or changing
settings ends that session. The worker exits, and the supervisor owns retries.

Internal modules have separate jobs:

- `config`: startup choices and negotiated native capture dimensions/rate.
- `control` and `events`: private supervisor IPC.
- `capture`, `native`, and `pixels`: acquire, release, and pack LG frames.
- `encoder`: raw/LZ4/H.264 encoding and reusable codec buffers.
- `transport`: bounded TCP negotiation and writes, with one owned socket.
- `session`: negotiation state, frame scheduling, and coordination.

Combined capture uses VTCapture's blended tap (mode 3), supplying video and
visible menus together as NV12. Video-only uses its display tap (mode 2).
These are the only two capture sources. DILE, GM/HAL, UI-only capture and CPU
blending have been removed. A poisoned constructor remains fatal. The native
adapter retains stride/geometry checks and bounded initialization retries.

## Capture and recovery

MAIN-sink readiness arrives from the service's Luna subscription. Unavailable
video defers acquisition and initialization for both sources. On the tested TV,
the blended tap cannot initialize on Home without active video; capture waits
until video returns. There is no separate UI fallback. Unknown readiness permits bounded native retry. Source/context changes
reset the video client, and rapid loss/return transitions remain latched until
consumed. Muted black video does not expire.

Native open, preparation, and release report operation start/completion to the
supervisor. Its five-second watchdog detects hangs, including cleanup after an
acquisition error; its subsequent TERM-to-KILL deadline bounds worker shutdown.
Initialization that remains unready for ten seconds reports failure before native
teardown. Native resource ownership stays on the capture thread; shutdown signals
are consumed by a separate waiting thread.

The tested TV has a reproduced native release hang. Process isolation remains
necessary; see the
[cleanup diagnosis](../../research/webos-capture/2026-11-01-lifecycle/cleanup-diagnosis.md).
These guards cannot make a blocking vendor call interruptible.

## Encoding and transport

The TV app offers **H.264 (software)** (`h264-software`). It adds **H.264
(hardware)** (`h264-hardware`) only when the supervisor reports
`capabilities.hardwareH264: true` in its status response. This describes build
support, not a runtime check of the TV's encoder. Default builds reject hardware
configuration. Hardware remains an explicit choice after the earlier native
binding implementation showed higher CPU use and an extra frame of latency.
See [hardware integration and measurements](../../docs/developers/hardware-h264.md).

Both capture sources pass native NV12 directly to the encoder. Combined capture
reports Blended content; video-only reports Video. Reusable buffers and checked,
tightly packed planes remain; failed acquisition never publishes stale pixels.

LZ4 compresses each frame independently and sends raw data when compression grows
it. H.264 uses one low-complexity encoder at a fixed 2 Mbit/s target, with limited
BT.709 conversion and rate-control skipping. Bitrate is not a strict instantaneous
limit. The receiver decodes every reference frame before publishing its latest
result. Each new session owns fresh codec state and begins with decoder
configuration and an IDR frame. Periodic IDRs are disabled for reliable TCP;
scene-change detection remains enabled.

There is no application frame queue. TCP connect and complete-frame writes have
100 ms deadlines; initial and partial control messages have a one-second bound.
The idle wait is interruptible. A partial/failed write disconnects immediately,
before potentially blocking native teardown. Kernel TCP buffering and
retransmissions can still increase latency.

The sole active version source is
[`wire-version.txt`](../../crates/capture/prysm-webos-protocol/wire-version.txt). It drives the JSON
hello version, four-byte frame magic, Rust fixtures, and TV smoke receiver. This
refactor does not change the receiver protocol. Frames retain the 24-byte header:

| Offset | Bytes | Field |
| --- | --- | --- |
| 0 | 4 | ASCII `PRY` and the version digit |
| 4 | 4 | Width, big-endian |
| 8 | 4 | Height, big-endian |
| 12 | 4 | Format: RGB24=0, NV12=1, NV16=2 |
| 16 | 4 | Codec: raw=0, LZ4=1, H.264=2 |
| 20 | 4 | Payload length, big-endian |

H.264 payloads are Annex B access units; the format field describes decoded RGB.
The receiver validates dimensions, lengths, codec data, and embedded H.264 SPS
before allocation/decoding. Limits remain 1920×1080, even capture dimensions,
1–60 FPS, and 8 MiB H.264 payloads. Mixed protocol versions are rejected.

The LAN protocol is unauthenticated and unencrypted. Restrict the listening
interface and port to the TV's trusted LAN. Packet/frame instrumentation remains
in the pipeline; the worker no longer exposes a standalone Prometheus CLI option.

## Verification

```sh
cargo test --locked -p prysm-webos-capture -p prysm-webos-protocol -p prysm-capture -p prysm-webos-service
node --test packaging/webos/tests/*.test.js
python3 -m unittest discover -s packaging/webos/tests
```

Tests cover worker-only startup, startup schema/identity boundaries, native ABI
and exception handling, pixel calibration, negotiated settings, idle cancellation,
readiness transitions, native cleanup watchdog ordering, codec framing and rate
control, write deadlines, service transactions, and authorization. Host fixtures
do not load real LG drivers or establish TV performance. Cross-build separately
with the SDK; use the bounded packaged smoke test for live hardware checks.
