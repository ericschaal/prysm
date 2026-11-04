# webOS capturer

The webOS capturer runs as a native Rust worker, captures video on a rooted LG TV and sends it to a
separate Prysm host. The [Luna supervisor](../prysm-webos-service/README.md)
starts it; there is no standalone mode or worker CLI. For installation, use
[TV setup](../../docs/users/webos.md).

The qualified device is LG 43QNED87A6D.BEUFLJP, webOS 25 (10.3.2-34). Native YUV
is limited-range BT.709, independently of output resolution. HDR capture is not
automatically tone-mapped. Root access alone does not qualify another model.

## Build for webOS

Use the [package build](../../docs/developers/packaging.md#webos) with a relocated
buildroot-nc4 SDK. It builds both binaries together. Executables go to
`target/armv7-unknown-linux-gnueabi/webos/`; the IPK goes to `target/webos-ipk/`.

The target is ARMv7/Cortex-A9 **soft-float** (`gnueabi`, not `gnueabihf`), with
an explicit SDK sysroot, optimized `webos` profile and `/lib/ld-linux.so.3` loader.
The SDK's glibc 2.12 needs `libglibc_polyfills.a` for `getauxval`. LG capture
libraries, `libstdc++.so.6` and `libgcc_s.so.1` come from the TV.

Hardware encoding requires `--hardware-h264` for both binaries and the TV's
GStreamer/GLib shared libraries. See the
[hardware encoder contract](../../docs/developers/hardware-h264.md).

## Worker startup and ownership

The supervisor launches without arguments and supplies:

| Channel | Contract |
| --- | --- |
| `PRYSM_WORKER_CONFIG` | Bounded typed JSON: validated HTTP(S) instance origin, source (`video`/`both`), compression (`none`/`lz4`/`h264-software`/`h264-hardware`) and required metrics boolean. |
| `PRYSM_EVENTS_FD` | Owned inherited pipe for typed lifecycle events. |
| stdin | Required private readiness/reset control pipe. |

Shared types live in `prysm-webos-protocol`. Reject invalid startup data or missing
channels before native capture. Run as root at the path authorized by the package's
capture role. The supervisor owns reconnect/backoff; one worker owns one connection
and exits when that session ends.

`config` handles startup/negotiation; `control`/`events` own IPC; `capture`,
`native` and `pixels` own acquisition and packing; `encoder` owns codec buffers;
`transport` owns the bounded socket; `session` coordinates them.

## Capture and recovery

Video-only uses VTCapture's display tap (mode 2); combined uses its blended tap
(mode 3), including visible menus. Both provide native NV12 and require active
video. Luna MAIN-sink readiness defers acquisition when video is absent. Unknown
readiness allows bounded native retry; context changes reset the client and rapid
loss/return transitions remain latched. Valid black frames do not expire.

Native open, preparation and release send watchdog events. Operations have a
five-second deadline; preparation cannot stay unready beyond ten seconds. The
supervisor kills a worker if native cleanup blocks. Native resources stay on the
capture thread; a separate signal-waiting thread handles shutdown. Process
isolation remains necessary because vendor release can hang.

Prysm owns requested resolution/FPS. Connect and negotiate before opening devices;
idle connections open none. Failed acquisition never publishes stale pixels.
Preserve stride/geometry validation and fail the worker after a poisoned constructor.

## Encoding and transport

Software H.264 uses bundled OpenH264 at a 2 Mbit/s target with rate-control skipping;
this is not an instantaneous bandwidth cap. LZ4 frames are independent and use raw
payloads when compression grows them. Every session has fresh codec state and
starts with decoder configuration plus IDR. The receiver decodes all reference
frames before publishing the latest result.

There is no application frame queue. DNS/TCP/TLS/upgrade/initial reply share four
seconds. Idle replies renew every second; each complete reply has four seconds.
Unavailable video releases the connection. Complete frame writes have 100 ms;
a partial write disconnects before native teardown. Kernel buffering/retransmission
can still add latency. WS/WSS transport is unauthenticated; use a trusted network.

[`wire-version.txt`](../../crates/capture/prysm-webos-protocol/wire-version.txt)
is the protocol version source. Each binary message contains JSON control or one
frame with this 24-byte, big-endian header:

| Offset | Field |
| --- | --- |
| 0 | Four-byte magic: ASCII `PRY` plus version digit |
| 4 | Width, u32 |
| 8 | Height, u32 |
| 12 | Format: RGB24=0, NV12=1, NV16=2 |
| 16 | Codec: raw=0, LZ4=1, H.264=2 |
| 20 | Payload length, u32 |

H.264 payloads are Annex B access units; their format describes decoded RGB.
Validate dimensions, lengths and SPS before allocation/decoding. Bounds are
1920×1080, even capture dimensions, 1–60 FPS and 8 MiB H.264 payloads.

## Verification

```sh
cargo test --locked -p prysm-webos-capture -p prysm-webos-protocol -p prysm-capture -p prysm-webos-service
python3 -m unittest discover -s packaging/webos/tests
```

TLS tests need the OpenSSL CLI for temporary loopback certificates. Fixtures cover
startup, ABI/exception handling, pixels, readiness, watchdogs, framing and deadlines;
they do not load LG drivers. Cross-build and run the
[bounded TV smoke test](../prysm-webos-service/README.md#isolated-hardware-test) separately.

Native adapters derive from MIT-licensed hyperion-webos revision
`00c932f092084b234b3f619b060a98fa134c12bd`; retain its notices. Vendor C++ exceptions
must not cross Rust FFI. Unsafe code stays in LG adapters, DMA mappings and inherited
event descriptors; GStreamer ownership uses typed bindings.
