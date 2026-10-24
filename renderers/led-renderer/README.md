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

Edit [`prysm.toml`](../../prysm.toml) and run from the workspace root:

```sh
cargo run --release -p prysm
# Or select another configuration file:
cargo run --release -p prysm -- --config "/path/to/prysm.toml"
```

For Raspberry Pi OS arm64, see the [Debian package and systemd service instructions](../../prysm/debian/README.md).

Configuration is read at startup. For file input and WLED output:

```toml
brightness_percent = 100
sample_density = 128
capture_resolution = [640, 360]

[input]
type = "file"
path = "/path/to/video.mp4"

[output]
type = "wled"
address = "wled.local:4048"
refresh_rate_hz = 30

[output.leds]
top = 96
right = 54
bottom = 96
left = 54
```

For device input, replace `[input]` with:

```toml
[input]
type = "device"
device = "/dev/video2" # Linux device path, or a macOS AVFoundation device UID
```

Both the file path and device value are required for their respective input types.
Relative file paths are resolved from the current working directory.
For an on-screen egui preview, replace the entire output configuration with:

```toml
[output]
type = "egui"
refresh_rate_hz = 30
```

For egui, `output.refresh_rate_hz` sets the preview refresh rate (positive integer,
default 30). Each refresh uses the latest frame and colors, skipping intermediate updates.

Both outputs use the configured brightness, sample density and capture resolution.
Brightness defaults to 100 and is clamped to 0–100. Sample density defaults to 128
samples per 1000 pixels of cropped edge length. Resolution defaults to `[640, 360]`;
devices must support the requested size. The four WLED edge counts may be zero,
but their total must be 1–65535. Wiring starts at bottom-left viewed from the front,
going up the left, across the top, down the right, and back along the bottom.
Unknown fields, missing required fields, empty paths/devices, invalid counts and
fields belonging to the other input/output type are rejected.

`prysm --help` describes the configuration option. Restart the app after changing
configuration. Ctrl+C stops processing; device, decoder and send failures exit
with an error. Closing the egui window also stops capture.
WLED restores its effect after its realtime timeout. File input requires FFmpeg
9+, converts HDR to sRGB, runs at the source frame rate, and exits successfully at EOF.

`output.refresh_rate_hz` caps WLED sends per second (positive integer, default 30).
Library callers can use `set_refresh_rate_hz(NonZeroU32)` to change the same cap,
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

Run the loopback packet and mapping checks with `cargo test -p led-renderer`.
