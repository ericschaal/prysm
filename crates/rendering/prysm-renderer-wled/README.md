# WLED renderer

`WledRenderer` sends RGB over DDP/UDP port 4048. Power/effects belong to the
[application](../../../docs/developers/control-api.md#lighting-transitions).

```rust
let mut renderer = WledRenderer::new("wled.local:4048")?;
renderer.render_edges(&colors, [96, 54, 96, 54])?;
```

Counts: `[top, right, bottom, left]`. Output starts bottom-left, clockwise viewed
from the front. Zero skips an edge. `render` accepts custom physical LED order.
Interpolation uses linear light; output is sRGB. WLED owns physical channel order
and gamma: enable **Use Gamma for color**, leave **Disable realtime gamma
correction** unchecked. [Setup](../../../docs/users/configuration.md).

`new` can block on DNS. Rendering is nonblocking with no queue/retry; preserve
`DDPError`/`WouldBlock`. UDP success means local acceptance. Keep sending static
frames for realtime mode; empty frames send nothing and shorter frames leave
trailing LEDs unchanged. Limit: 65535 LEDs.

`cargo test -p prysm-renderer-wled`
