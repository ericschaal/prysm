<p align="center">
  <img src="docs/images/hero.svg" alt="Prysm — beyond the screen. A twilight landscape extends into a soft pink, violet, blue, and teal glow around a television." width="100%">
</p>

<p align="center">
  <strong>An Ambilight system for your screen.</strong><br>
  Prysm syncs LED strips behind your screen with the colors in your video.<br>
  Send colors to WLED and control the lights from your phone.
</p>

<p align="center">
  <a href="#in-action">In action</a> ·
  <a href="#what-it-does">What it does</a> ·
  <a href="#get-started">Get started</a> ·
  <a href="#set-the-mood">Control panel</a> ·
  <a href="#under-the-hood">Under the hood</a>
</p>

## In action

<p align="center">
  <img src="docs/images/ambilight-action.webp" alt="Prysm lighting the wall around a television in red and blue as the colors move across an action scene." width="768">
  <br><sub>Red and blue light follows the action across the screen.</sub>
</p>

<p align="center">
  <img src="docs/images/ambilight-music.webp" alt="Prysm lighting the wall around a television as a music video shifts between cool stadium lighting and warm outdoor scenes." width="768">
  <br><sub>Cool stadium light shifts to warm colors with the music video.</sub>
</p>

## What it does

Prysm captures your live video feed, reads the colors along each edge, and sends them to LED strips driven by WLED. The LEDs light the wall behind your screen with colors that follow the picture as it changes.

| | |
| :--- | :--- |
| **Colors matched to each edge** | Each section of the LED strip follows the corresponding part of the picture. |
| **Smooth transitions** | Color changes are softened over time to reduce flicker. |
| **Black-bar detection** | Letterboxed video is sampled inside the black bars, so the lights follow the image. |
| **Screen sync or WLED effects** | Use the phone panel to switch sources or turn the lights off. |
| **Local control** | The panel runs on your Pi and works over your local network without a cloud account. |

## Set the mood

<p align="center">
  <img src="docs/images/control-panel.jpg" alt="Prysm control panel showing Set the mood, lights On, Screen sync selected, a WLED effects option, and Customize in WLED." width="400">
</p>

<p align="center"><sub>The actual control interface, shown with demo connection state.</sub></p>

One power button. Two color sources. A quiet status indicator.

- **Screen sync** lets Prysm supply live colors from your video.
- **WLED effects** returns control to WLED's effects and presets.
- **Customize in WLED** opens WLED's own interface for the finer details.

The panel remembers your selected source while the lights are off. It stays available when capture stops, and shows connection problems with retry controls. Keyboard navigation, visible focus, generous touch targets, and reduced-motion support are built in.

The controller requires Linux with systemd and a compatible WLED JSON API. See the [control-panel guide](docs/control-panel.md) for behavior and configuration.

## How it connects

```text
Capture device → Prysm → DDP over your LAN → WLED → LED strips
                   ↑
            Phone control panel
```

Prysm handles capture and color processing. WLED drives the physical LEDs. The phone panel switches the capture service and WLED's power/source state.

### What you need

- A Linux or macOS machine for capture and processing; a Raspberry Pi with 64-bit Raspberry Pi OS works for the packaged setup.
- A compatible video capture device, such as a USB capture card exposing your video feed.
- A WLED controller, addressable LED strips, a suitable power supply, and a local network connection.
- For building: a Rust toolchain supporting edition 2024.

Capture uses V4L2 on Linux and AVFoundation on macOS. It reads from a video capture device; it does not capture your desktop directly. The browser controller is Linux-only.

## Get started

### 1. Build

```sh
git clone https://github.com/ericschaal/prysm.git
cd prysm
cargo build --locked --release -p prysm --bins
```

On Linux, install the native capture and graphics build dependencies first; the [Raspberry Pi / Debian guide](prysm/debian/README.md) lists them. Desktop dependencies are currently compiled for the headless application too.

### 2. Configure your setup

Edit [`prysm.toml`](prysm.toml). The checked-in values describe a specific setup; replace the device, WLED address, and LED counts with yours. For example:

```toml
brightness_percent = 100
sample_density = 128
capture_resolution = [640, 360]

[input]
type = "device"
device = "/dev/video0" # On macOS, use the AVFoundation device UID.

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

LED counts refer to the screen viewed from the front. Wiring starts at **bottom-left**, then runs up the left, across the top, down the right, and back along the bottom. Set an absent edge to `0`.

In WLED, enable DDP input under **Config → Sync Interfaces** and match the LED count to your strip. Prysm sends sRGB colors: enable **Use Gamma for color** and leave **Disable realtime gamma correction** unchecked. The [WLED setup guide](renderers/led-renderer/README.md) covers mapping, brightness, and transport details.

### 3. Turn it on

```sh
cargo run --locked --release -p prysm
# Or load another configuration:
cargo run --locked --release -p prysm -- --config /path/to/prysm.toml
```

Configuration is read at startup; restart after editing it. `Ctrl+C` stops processing. Once live input times out, WLED returns to its normal effect.

## Put it on a Raspberry Pi

The Debian package installs the capture application, the independent web controller, and their systemd units. Follow the [build and installation guide](prysm/debian/README.md) first; services are disabled on installation so you can configure your hardware.

To let the panel manage screen sync after installing and configuring the package:

```sh
sudo systemctl disable --now prysm-led
sudo nano /etc/prysm/control.toml
sudo systemctl enable --now prysm-control
```

Set `wled_url` to WLED's **HTTP address**, such as `http://wled.local/`; the capture output separately uses DDP port `4048`. Configure WLED's boot preset to Off, then open `http://YOUR_PI_IP:8787` on your phone.

The panel has no login: use a trusted LAN or private VPN and keep its port off the public internet. Off is a software lighting command, not a physical power disconnect. Verify power and source switching on your installed WLED firmware before relying on it.

## Under the hood

Prysm is a Rust workspace with a straightforward **capture → process → render** pipeline.

| Component | Responsibility |
| :--- | :--- |
| [`prysm`](prysm/) | Configuration, application, web controller, and file input for debugging. |
| [`prysm-capture`](prysm-capture/) | Linux V4L2 and macOS AVFoundation capture. |
| [`prysm-core`](prysm-core/) | Colors, gradients, and processing settings; no external dependencies. |
| [`prysm-processor`](prysm-processor/) | Black-bar detection, edge sampling, and temporal smoothing. |
| [`desktop-renderer`](renderers/desktop-renderer/) | On-screen egui preview for debugging. |
| [`led-renderer`](renderers/led-renderer/) | WLED output over DDP/UDP with a configurable send-rate cap. |

Processing settings such as smoothing and sampling depth are available through the core library; the application's TOML exposes brightness, sample density, capture resolution, and input/output settings.

### Development

```sh
cargo test --workspace --locked
cargo fmt --all -- --check
```

For control-panel changes, use Node 20.19+ and the existing frontend tooling:

```sh
npm ci --prefix prysm/control-ui
# Edit prysm/control-ui/app.jsx or style.css, then:
npm run build --prefix prysm/control-ui
npm test --prefix prysm/control-ui
```

Commit the generated `prysm/src/control.html` alongside its source changes. The deployed panel needs no Node runtime.

### Desktop debugging

The egui renderer shows the captured video and sampled colors so you can inspect the processing pipeline. File input is a debugging tool, usually paired with this renderer to replay recorded footage.

For a recorded-video preview, replace `[input]` and the entire `[output]` configuration, including `[output.leds]`, with:

```toml
[input]
type = "file"
path = "/path/to/video.mp4"

[output]
type = "egui"
refresh_rate_hz = 30
```

`refresh_rate_hz` sets the preview refresh rate (positive integer, default 30).
Each refresh displays the latest available frame and colors; intermediate updates are skipped.

Run `cargo run --locked --release -p prysm`. File input requires **FFmpeg 9+** on your `PATH` and a readable video file. Playback runs at the source frame rate, converts HDR to sRGB, and stops at the end of the video. Relative file paths resolve from your working directory. To inspect live capture instead, keep your device input and use the same egui output.

## If something looks off

| Symptom | Check |
| :--- | :--- |
| Capture won't start | Device path/UID, permissions, and support for the configured resolution. On Linux, `/dev/v4l/by-id/` provides stable device paths. |
| LEDs stay dark | WLED address, DDP input, LED counts, power, and the selected source. |
| Colors look washed out | WLED's gamma settings described above; avoid applying gamma twice. |
| Panel shows Unknown | WLED connectivity and controller logs: `journalctl -u prysm-control -f`. |

For more detail: [WLED output](renderers/led-renderer/README.md) · [Pi deployment](prysm/debian/README.md) · [Control panel](docs/control-panel.md) · [Visual A/B testing](docs/ambilight-ab-testing.md) · [Error handling](docs/error-handling.md).
