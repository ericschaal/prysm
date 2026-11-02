# WLED renderer

`WledRenderer` sends RGB frames through `ddp-rs` over UDP to one WLED instance.
DDP is the preferred transport here: compact binary frames, automatic splitting
at 480 RGB LEDs per packet, and a push flag on the last packet. WLED listens on
port 4048. See [WLED's DDP documentation](https://kno.wled.ge/interfaces/ddp/).

```rust
use prysm_renderer_wled::WledRenderer;
use prysm_core::{Color, EdgeColors};

let mut renderer = WledRenderer::new("wled.local:4048")?;
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

Use a gamma near **2.2** as a nominal starting point for sRGB input, then
calibrate against your LEDs and wall. A steeper curve such as 2.8 preserves
maximum white but substantially darkens shadows and midtones: an input channel
of 128 becomes drive 37 at 2.8, versus 56 at 2.2 (exact sRGB decoding gives 55).
See the [HDR and brightness investigation](../../../research/webos-capture/2026-11-01-hdr-brightness/README.md).

For application setup, input selection and lifecycle behavior, use the
[capture quickstart](../../../docs/users/quickstart.md),
[configuration guide](../../../docs/users/configuration.md), and
[control API](../../../docs/developers/control-api.md).

The renderer sends every nonempty frame immediately. Frames use a
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
