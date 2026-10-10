# WLED renderer

`WledRenderer` sends RGB frames through `ddp-rs` over UDP to one WLED instance.
DDP is the preferred transport here: compact binary frames, automatic splitting
at 480 RGB LEDs per packet, and a push flag on the last packet. WLED listens on
port 4048. See [WLED's DDP documentation](https://kno.wled.ge/interfaces/ddp/).

```rust
use led_renderer::WledRenderer;
use prysm_core::{Color, EdgeColors};

let mut renderer = WledRenderer::new("wled.local:4048")?;
renderer.render(&[Color::new(255, 0, 0), Color::new(0, 0, 255)])?;

// Call for each processor output; counts match the physical strip.
renderer.render_edges(&EdgeColors::default(), [96, 54, 96, 54])?;
```

`render` accepts colors in physical LED order. `render_edges` resamples the
processor's linear gradients and encodes them as 8-bit sRGB. It starts at the
top-left corner and runs clockwise as viewed from the front: top, right,
bottom, left. Counts can be zero for absent edges. Bottom and left gradients
are reversed because processor samples run left-to-right and top-to-bottom.
For a different starting point or direction, assemble your colors and use `render`.

Enable DDP input under WLED's **Config → Sync Interfaces**, saving and rebooting
if needed. Match WLED's LED count to your frame length and set its realtime
brightness, gamma, mapping, and timeout for your hardware. The renderer sends
RGB channels; WLED handles the strip's physical color order. It does not change
the device configuration.

Run camera capture directly to WLED without a desktop window:

```sh
cargo run --release -p prysm --bin led -- wled.local:4048 96 54 96 54
# Optionally select a capture device (Linux path or macOS AVFoundation UID):
cargo run --release -p prysm --bin led -- wled.local:4048 96 54 96 54 /dev/video2
# Or play the local Philips HDR test clip with FFmpeg 9+:
cargo run --release -p prysm --bin led -- wled.local:4048 96 54 96 54 --video
# Or another video file:
cargo run --release -p prysm --bin led -- wled.local:4048 96 54 96 54 --video "/path/to/video.mp4"
```

The four counts are top, right, bottom, left; the total must be 1–65535.
Both the `led` and desktop `video` binaries use `clap`; run them with `--help`
for usage. The optional camera device and `--video` are mutually exclusive.
The binary uses the same 640×360 capture and default processor settings as the
desktop app. Ctrl+C stops capture and transmission; capture or send failures
exit with an error. WLED restores its effect after its realtime timeout.
Video input uses the same FFmpeg decoding and HDR-to-sRGB conversion as the
desktop video player, runs at the source frame rate, and exits successfully at EOF.

Frames are sent immediately on a nonblocking socket, with no queue or retries;
the RGB buffer is reused. DNS resolution happens in the constructor and can
block. Render errors are returned as `ddp_rs::error::DDPError` (including an I/O
`WouldBlock` if the send buffer fills). A failed multi-packet send can leave a
partial update; send the next full frame. UDP does not acknowledge delivery.

Keep sending at your source frame rate, including static frames, to keep WLED in
realtime mode. Stopping lets WLED's configured timeout restore its normal effect.
Empty frames send nothing; shrinking a frame does not clear trailing LEDs, so
keep the configured length fixed. Frames above 65535 LEDs are rejected.

Run the loopback packet and mapping checks with `cargo test -p led-renderer`.
