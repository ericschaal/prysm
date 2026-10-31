# WLED renderer

`WledRenderer` sends RGB frames through `ddp-rs` over UDP to one WLED instance.
DDP is the preferred transport here: compact binary frames, automatic splitting
at 480 RGB LEDs per packet, and a push flag on the last packet. WLED listens on
port 4048. See [WLED's DDP documentation](https://kno.wled.ge/interfaces/ddp/).

```rust
use prysm_renderer_wled::WledRenderer;
use prysm_core::{Color, EdgeColors};
use std::num::NonZeroU32;

let mut renderer = WledRenderer::new("wled.local:4048", NonZeroU32::new(30).unwrap())?;
renderer.render(&[Color::new(255, 0, 0), Color::new(0, 0, 255)])?;

// Call for each processor output; counts match the physical strip.
renderer.render_edges(&EdgeColors::default(), [96, 54, 96, 54])?;
```

`render` accepts colors in physical LED order. `render_edges` resamples the
processor's linear gradients and encodes them as 8-bit sRGB. It starts at the
bottom-left corner and runs clockwise as viewed from the front: left bottom-to-top,
top left-to-right, right top-to-bottom, bottom right-to-left.
Counts can be zero for absent edges. Bottom and left gradients
are reversed because processor samples run left-to-right and top-to-bottom.
For a different starting point or direction, assemble your colors and use `render`.

Enable DDP input under WLED's **Config → Sync Interfaces**, saving and rebooting
if needed. Match WLED's LED count to your frame length and set its realtime
brightness, gamma, mapping, and timeout for your hardware. The renderer sends
RGB channels; WLED handles the strip's physical color order. It does not change
the device configuration.

Prysm sends sRGB, so enable **Use Gamma for color** in **LED Preferences** and
leave **Disable realtime gamma correction** unchecked in **Sync Interfaces**.
Bypassing gamma drives LEDs with sRGB values as raw brightness, lifting dark
channels and making dark blues look cyan or washed out. Gamma correction belongs
in WLED; do not add a second correction in Prysm.

Edit [`prysm.toml`](../../../prysm.toml) and run from the workspace root:

```sh
cargo run --release -p prysm
# Or select another configuration file:
cargo run --release -p prysm -- --config "/path/to/prysm.toml"
```

For Raspberry Pi OS arm64, see the [Debian package and systemd service instructions](../../../packaging/debian/README.md).

Start the controller, select Screen sync, and turn the lights on in the panel.
All settings are owned by the controller's single TOML file. File input uses:

```toml
[input]
type = "file"
path = "/path/to/video.mp4"
```

Device input uses `type = "device"` with a Linux device path or macOS AVFoundation
UID in `device`. Select `[output] type = "wled"` and configure its `address`, positive
`refresh_rate_hz`, and top/right/bottom/left LED counts. Alternatively, select
`type = "egui"` with `--features desktop` for a native output without WLED.
Counts may be zero for absent edges, with a total of 1–65535 LEDs. Processing
settings belong under `[processing]`. See the complete [example](../../../prysm.toml)
and [controller guide](../../../docs/control-panel.md).

Settings saved through HTTP are persisted atomically. Processing changes apply
live; input, resolution, and output changes recreate a running actor session.
File EOF completes capture, while capture/decoder/send failures leave HTTP alive
for inspection and retry. Ctrl+C drains the pipeline and exits. File input needs
FFmpeg 9+, converts HDR to sRGB, and runs at the source frame rate.

`output.refresh_rate_hz` caps WLED sends per second (positive integer).
Library callers pass this cap as a `NonZeroU32` to `WledRenderer::new`,
shared by `render` and `render_edges`. The source frame rate limits actual output;
skipped frames are not queued. The first nonempty frame sends immediately; calls
within the configured interval of the previous successful send are skipped and
return success. Frames use a
nonblocking socket with no queue or retries; the RGB buffer is reused.
DNS resolution happens in the constructor and can
block. Render errors are returned as `ddp_rs::error::DDPError` (including an I/O
`WouldBlock` if the send buffer fills). A failed multi-packet send can leave a
partial update; send the next full frame. UDP does not acknowledge delivery.

Keep sending at your source frame rate, including static frames, to keep WLED in
realtime mode. Stopping lets WLED's configured timeout restore its normal effect.
Empty frames send nothing; shrinking a frame does not clear trailing LEDs, so
keep the configured length fixed. Frames above 65535 LEDs are rejected.

Run the loopback packet and mapping checks with `cargo test -p prysm-renderer-wled`.
