# Experimental hardware H.264

`cargo xtask package webos --hardware-h264` enables both binaries. The UI capability
reports build support, not a working encoder. Default builds use software H.264.

## Native boundary

- Input: native limited-range BT.709 NV12. Linux links to TV GStreamer/GLib and
  vendor plugins; these are required even when software encoding is selected.
- `rvenc` → `appsink`, without `h264parse`. Keep two persistent DMA-BUFs with
  192-byte pitch; pair cache synchronization and reuse only after GStreamer release.
- One picture in flight. Validate Annex B access units; reject B/SP/SI slices and
  multiple pictures. SPS edits change only crop/video-signal metadata.
- Fresh session starts SPS/PPS/IDR. Failure after transmitting a reference closes
  the session before software fallback; never carry references across sessions.
- Typed bindings own GStreamer resources; DMA descriptors/mappings stay private
  to [gst.rs](../../apps/prysm-webos-capture/src/encoder/gst.rs).
- Output/EOS waits: 1 s/500 ms. Supervisor watchdog: 5 s plus kill deadline.
  A killed worker cannot perform in-process fallback.

## Checks

```sh
cargo test --locked -p prysm-webos-capture -p prysm-webos-service -p prysm-webos-protocol -p prysm-capture
```

On Linux with GStreamer development files, repeat with
`--features prysm-webos-capture/hardware-h264,prysm-webos-service/hardware-h264`.
macOS uses a stub. Validate changing pictures and cleanup with [TV checks](debugging.md#hardware-checks).
