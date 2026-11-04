# WLED renderer

`WledRenderer` sends RGB colors to one WLED instance over DDP/UDP port 4048,
using `ddp-rs`. Packets split at 480 RGB LEDs, with a push flag on the last packet.
WLED is Prysm's only LED renderer. Application power/effects control lives outside
this crate; see [lighting transitions](../../../docs/developers/control-api.md#lighting-transitions).

```rust
use prysm_renderer_wled::WledRenderer;
use prysm_core::{Color, EdgeColors};

let mut renderer = WledRenderer::new("wled.local:4048")?;
renderer.render(&[Color::new(255, 0, 0), Color::new(0, 0, 255)])?;
renderer.render_edges(&EdgeColors::default(), [96, 54, 96, 54])?;
```

## LED order and color

`render` takes physical LED order. `render_edges` takes counts in
`[top, right, bottom, left]` order, resamples linear gradients and encodes sRGB.
Physical output starts bottom-left and runs clockwise as seen from the front:
left bottom-to-top → top left-to-right → right top-to-bottom → bottom right-to-left.
Bottom/left samples reverse because processor arrays use screen coordinates.
Zero counts omit edges. For another start/direction, arrange colors and use `render`.

WLED must have DDP enabled and its LED count configured. The renderer sends RGB;
WLED owns physical channel order, gamma, mapping and timeout. Prysm sends sRGB:
enable WLED **Use Gamma for color** and leave **Disable realtime gamma correction**
unchecked. Do not add a second gamma correction here. See
[user configuration](../../../docs/users/configuration.md) for setup.

## Failure and timing

DNS resolution in `new` can block; the application does this off its control
runtime. Rendering reuses the RGB buffer and sends immediately through a
nonblocking socket, without queue/retry. Errors remain `DDPError`, including
`WouldBlock`. A partial multi-packet send can leave a partial update; the next full
frame replaces it. UDP socket acceptance does not confirm delivery.

Keep sending static frames to retain realtime mode. Empty frames send nothing;
shorter frames do not clear trailing LEDs, so keep length fixed. More than 65535
LEDs is rejected. Stopping lets WLED's timeout restore effects; the application
also explicitly controls power/source after draining output.

Run packet/mapping tests with `cargo test -p prysm-renderer-wled`.
