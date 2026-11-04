# Experimental webOS hardware H.264

Hardware encoding is opt-in: `cargo xtask package webos --hardware-h264` enables
it in both worker and supervisor. The TV UI offers it only when
`capabilities.hardwareH264` is true. That reports build support, not a working
encoder on the current TV. Default builds use software H.264.

## Native boundary

Both capture modes supply native limited-range BT.709 NV12. On Linux the backend
links dynamically to the TV's GStreamer core, app, video, allocators and GLib.
Those libraries are required even if software compression is selected in a
hardware-enabled package. The SDK supplies target pkg-config metadata; libraries
and vendor plugins are not bundled. Non-Linux tests use an unavailable-backend stub.

`rvenc` connects directly to `appsink`. Two persistent, CPU-mapped NV12 DMA-BUFs
use 192-byte pitch. Padding is initialized once; uploads preserve it and pair
cache synchronization around every CPU write. Reuse a slot only after GStreamer
releases its input reference. Per-frame allocation is unsafe for changing pictures
on the tested hardware.

There is one submitted picture awaiting output. Avoid `h264parse`: it waits for
the next input on this TV. The vendor advertises AVC but emits Annex B, so do not
force output caps. Validate actual access units and reject B/SP/SI slices and
multiple pictures. SPS correction changes only crop and video-signal metadata;
the receiver still validates dimensions, sequence, lengths and limited BT.709.

Each session begins with SPS/PPS/IDR and owns fresh codec state. Initialization
or first-frame failure may fall back before transmitting a hardware reference.
Later failure closes the session and starts a new software session; reference
state must never cross that boundary.

Typed GStreamer bindings own pipelines, buffers and samples. DMA allocation,
descriptors and mappings stay private to `encoder/gst.rs`; callers provide slices
and validated dimensions, never pointers/capacities. There are no handwritten
Send/Sync implementations or Rust callbacks.

Output waits are bounded to one second and EOS to 500 ms. Native creation,
teardown and NULL transitions also have the supervisor's five-second watchdog
and kill deadline. A blocking vendor call is not interruptible; a killed worker
cannot perform fallback, and its replacement may attempt hardware again.

## Checks

```sh
cargo test --locked -p prysm-webos-capture -p prysm-webos-service -p prysm-webos-protocol -p prysm-capture
cargo clippy --locked -p prysm-webos-capture -p prysm-webos-service -p prysm-webos-protocol --all-targets -- -D warnings
```

On Linux with GStreamer development files, repeat with
`--features prysm-webos-capture/hardware-h264,prysm-webos-service/hardware-h264`.
macOS checks do not exercise the native backend. Hardware qualification requires
[TV checks](debugging.md#hardware-checks), including changing pictures and cleanup.
