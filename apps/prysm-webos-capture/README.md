# webOS capturer

Native Rust worker supervised by [prysm-webos-service](../prysm-webos-service/README.md).
Captures rooted LG TV video and sends it to a separate Prysm host.
[Install](../../docs/users/webos.md) · [Lifecycle](../../docs/developers/webos.md)

## Build for webOS

Use the [package task](../../docs/developers/packaging.md#webos). Target:
ARMv7/Cortex-A9 soft-float (`gnueabi`), SDK sysroot, `/lib/ld-linux.so.3` and
`libglibc_polyfills.a`. LG libraries come from the TV. Hardware encoding has
[extra dependencies](../../docs/developers/hardware-h264.md).

## Worker startup and ownership

The supervisor supplies validated `PRYSM_WORKER_CONFIG`, an inherited
`PRYSM_EVENTS_FD` event pipe and stdin readiness commands. Reject missing/invalid
channels before opening capture. Run at the installed role's authorized root path.
One worker owns one connection/session; the supervisor owns retries.
Shared [protocol types](../../crates/capture/prysm-webos-protocol/src/) define IPC.

## Capture and recovery

VTCapture mode 2 is video-only; mode 3 includes menus. Both yield NV12 and wait for
MAIN video readiness. Context changes reset native state. Never publish failed or
stale acquisitions. Native calls have watchdogs; cleanup can hang, so preserve
process isolation, hardware locking and bounded teardown.

Qualified hardware: LG 43QNED87A6D.BEUFLJP, webOS 25 (10.3.2-34). Capture is
limited-range BT.709; HDR is not automatically tone-mapped.

## Encoding and transport

Raw/LZ4 are independent frames; H.264 sessions start with configuration/IDR and
fresh codec state. Decode every reference before replacing the displayed frame.
No application frame queue. Connect/handshake: 4 s; idle replies: 1 s renewal/4 s
deadline; frame writes: 100 ms. Partial writes disconnect before native cleanup.

The [protocol source](../../crates/capture/prysm-webos-protocol/src/frame.rs) owns
headers/validation. Bounds: 1920×1080, even dimensions, 1–60 FPS, 8 MiB H.264.
Use trusted networking; WS/WSS transport has no authentication.

## Verification

```sh
cargo test --locked -p prysm-webos-capture -p prysm-webos-protocol -p prysm-capture -p prysm-webos-service
```

TLS tests need OpenSSL CLI. Follow with [TV smoke tests](../prysm-webos-service/README.md#isolated-hardware-test).
Keep MIT adapter notices; vendor C++ exceptions must not cross Rust FFI.
