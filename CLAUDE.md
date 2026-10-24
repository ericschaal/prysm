# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Prysm** is an ambient lighting system (ambilight/bias lighting) that captures video from a camera, analyzes edge
colors, and generates color gradients for LED strips.

**Purpose:** Library ecosystem for capturing video, analyzing edge colors, and driving LED output.

**Current State:**

- Desktop demo/visualizer binary (Linux/V4L) is working
- Library components are modular and reusable
- Headless WLED output supports camera capture and FFmpeg video input
- Architecture supports multiple capture sources and rendering backends

**Long-term Goal:** Move toward `no_std` compatibility for embedded/microcontroller targets

- Currently uses std library (tokio, async I/O)
- Keep this goal in mind when adding dependencies or features
- `prysm-core` already has zero external dependencies as a step toward this goal

## Architecture Overview

Error handling policy: see [docs/error-handling.md](docs/error-handling.md).

### Workspace Structure

**6 crates, with one application binary in `prysm`**

Libraries are reusable components for capture, processing and rendering:

- `prysm` selects device/file input and WLED/egui output through TOML configuration
- Clean separation: capture → process → render
- Trait-based extensibility (not plugin-based)

### Key Architectural Details

**Threading Model:** Two-thread architecture

- Main thread: egui desktop renderer (blocking GUI)
- Runtime thread: Single-threaded Tokio runtime for async I/O
- Communication via `tokio::sync::watch` channels (async→sync bridge)

**Data Flow Pipeline:**

```
Camera / FFmpeg → Frame Stream → owned desktop consumer
                                  ↓                ↓
                             PrysmProcessor   Frame watch
                                  ↓                ↓
                             Colors watch → DesktopRenderer

Camera / FFmpeg → Frame Stream → PrysmProcessor → WledRenderer
```

**Core Abstractions:**

- `PrysmCapturer` trait: Extensible video capture interface
- `publish_frames()`: Owns desktop processing, watch publication, and cancellation
- Desktop consumes each delivered frame once; both camera adapters retain only the newest pending frame
- Sources yield `Result<Frame>`: camera failures are terminal errors, successful file EOF is normal completion
- Slow camera consumers skip intermediate frames; smoothing and black-bar confirmation use elapsed processing time

### Crate Responsibilities

**Libraries (reusable components):**

- `prysm-core`: Data structures (Color, ColorStrip, EdgeColors, Config) - zero external dependencies
- `prysm-capture`: Frame abstraction, PixelFormat enum, PrysmCapturer trait
- `prysm-processor`: Video analysis → edge color strips (stateful with temporal smoothing)
- `v4l-capturer`: Linux V4L2 video capture implementation
- `desktop-renderer`: egui/eframe GUI for visualization
- `led-renderer`: WLED RGB output over DDP/UDP using `ddp-rs`

**Binary:**

- `prysm`: Device or file input → processor → WLED or egui, selected by TOML.

## Common Development Commands

```bash
# Build
cargo build
cargo build --release

# Run the configured input and output from prysm.toml
cargo run -p prysm
# Or select another configuration
cargo run --release -p prysm -- --config /path/to/prysm.toml

# Run tests
cargo test
cargo test -p prysm-capture  # Run tests in specific crate

# Code quality
cargo check
cargo fmt
cargo clippy
```

## Key Configuration Points

**Application setup (`prysm/src/config.rs`):**

The single `prysm` binary reads `prysm.toml` at startup (or `--config PATH`).
Top-level `brightness_percent`, `sample_density`, `spatial_reach_percent`, and `capture_resolution` default
respectively to 100, 128 samples per 1000 edge pixels, 10% of picture height, and `[640, 360]`.
These processor settings apply to both outputs.

`[input]` selects `type = "device"` with a required `device` (Linux device path
or macOS AVFoundation UID), or `type = "file"` with a required `path`.
File input uses FFmpeg 9+ for HDR-to-sRGB conversion, plays at the source rate,
and exits successfully at EOF. Relative paths use the current working directory.

`[output]` selects `type = "wled"` with an `address` (DDP port 4048) and
`[output.leds]` counts for top/right/bottom/left, or `type = "egui"` for an
on-screen preview. WLED counts total 1–65535; wiring starts at bottom-left viewed
from the front, up the left, across the top, down the right, and back along the bottom.
The desktop output retains the main-thread egui / runtime-thread arrangement in
`prysm/src/desktop.rs`; WLED runs headlessly on a single Tokio runtime in
`prysm/src/wled.rs`.
Ctrl+C cancels either output; closing egui also stops capture. Source and renderer
failures preserve their error context. WLED's realtime timeout restores its normal effect.

The current pipeline assumes sRGB. `LinearColor` uses floating-point math, but input decoding
uses the sRGB transfer function and preview output is 8-bit RGB. BT.2020 YUV matrix support
does not include PQ/HLG transfer decoding or an HDR display path, so file playback tone-maps first.

**Default Config (prysm_core::Config):**

Application settings are parsed into one core `Config` in `prysm/src/config.rs`.
That config owns capture resolution and processing settings; egui and input creation
read the same resolution from it.
All numeric defaults live in `prysm-core/src/config.rs`; the TOML parser uses the
domain types’ `Default` implementations. Brightness, spatial reach, smoothing duration,
and capture resolution use distinct newtypes. Resolution, input identifiers/paths
and WLED layouts are validated when parsed.
Library callers can pass a `Config` directly to `PrysmProcessor::new`.
Start with `Config::default()` and override only what you want to change:

```rust
let config = prysm_core::Config {
    brightness_percent: prysm_core::BrightnessPercent(60),
    smoothing_seconds: prysm_core::SmoothingSeconds(0.2),
    sample_density: prysm_core::SampleDensity(150),
    ..prysm_core::Config::default()
};
```

| Setting | Default | What it changes |
| --- | --- | --- |
| `capture_resolution` | `CaptureResolution::new(640, 360)` | Requested device capture or decoded file dimensions. |
| `brightness_percent` | 100 | 0 turns lights off; 100 is full brightness. |
| `smoothing_seconds` | 0.1 | Time to complete 95% of a transition, independent of frame rate. Zero disables smoothing. |
| `sample_density` | `SampleDensity(128)` | Color samples per 1000 pixels of cropped edge length, independent of LED count. |
| `spatial_reach_percent` | 10 | Broad influence radius as a percentage of cropped picture height, with a stronger local peak at one quarter of that radius. All picture pixels contribute. |
| `remove_black_bars` | true | Follow the picture inside stable black bars. Set false to sample the full frame. |

Brightness is clamped to 0–100, spatial reach to 1–100. Negative and non-finite smoothing durations
disable smoothing. Every frame is sampled; density sets output positions and the area-average grid
resolution, capped at 128 cells on its longest side. All picture pixels contribute.
Sample counts are capped at one per pixel, with at least one per edge. Black-bar tuning is internal.
The detector scans at most every 50 ms and confirms each axis independently after 200 ms
of stable evidence. It handles thin and combined bars, checks the capture's black level, and
preserves the confirmed crop through dark scenes. Row and column scans use the 98th percentile
so a bar must be almost entirely black; visible minority detail stops the crop. Cross-axis passes
exclude the other bars when inspecting narrow content. Opposing margins differing by more than
2.5% of the axis use their shared depth, favoring picture retention over removal of strongly
asymmetric bars. Pictures spanning less than 10% of either dimension or with sparse bright
content may remain undetected. Crop changes retain temporal smoothing, resampling its history
to the new edge sample counts.
`process_frame` uses monotonic processing time; deterministic replay can use `process_frame_at`
with a shared clock origin plus source timestamps.

`edge_depth_percent: EdgeDepthPercent` is replaced by `spatial_reach_percent: SpatialReachPercent`.
Reach controls continuous falloff rather than an edge-band cutoff.
The old fractional `brightness` field is replaced by an integer percentage;
`black_band_detection` is now `remove_black_bars`. `smoothing_percent` is replaced by
`smoothing_seconds`; `sample_density` is configurable again. Detector tuning stays internal.

## Testing Structure

Tests are minimal but focused:

- YUYV color conversion tests in `prysm-capture/src/yuyv.rs`
- Test black, white, gray, and color-tinted pixel conversions
- Verify output dimensions

## Important Implementation Details

### When adding new capture sources:

- Implement the `PrysmCapturer` trait
- Return a stream of `Result<Frame>` objects; preserve startup/runtime failure context
- Use the shared latest-frame capture channel; keep terminal results separate from replaceable frames
- Handle blocking I/O by spawning OS threads (see v4l-capturer pattern)
- Use Arc-wrapped frame data for zero-copy sharing

### When working with the processor:

- `PrysmProcessor` chains typed nodes: `BandDetector` (letterbox/pillarbox viewport) → `SpatialSampler`
  (whole-picture area averaging and distance-weighted projection onto the perimeter) → `TemporalSmoothing`
- Frames carry their YUYV range and matrix from capture into both processing and preview.
- Brightness scales the final edge colors after smoothing.
- Frames stay in their raw capture format end-to-end; there is no full-frame RGB decode. Each node
  area-averages raw pixels directly into a small linear RGB grid (`ViewFrame::average_linear`).
  Spatial weights are cached by viewport dimensions. Band detection reads luma via
  `frames::luma_at`.
- The processor is stateful (smoothing history, band debounce)
- Supports YUYV, RGB24, and BGR24; MJPEG currently returns black edge colors
- `cargo run --release -p prysm-processor --example bench` gives rough per-frame pipeline cost

### Threading considerations:

- Recent change (commit `24e5ddd`): Tokio runtime runs on single thread for efficiency
- Don't block the async runtime - use spawn_blocking or OS threads for blocking operations
- Watch channels enable non-blocking UI updates from async streams

### Dependency management:

- Future goal: `no_std` compatibility for embedded targets
- Keep dependencies minimal, especially in core libraries
- `prysm-core` intentionally has zero external dependencies
- Consider embedded/no_std compatibility when adding new dependencies or features

## Critical Files

- `prysm/src/main.rs` - CLI arguments and logging setup
- `prysm/src/app.rs` - TOML-driven application setup and output selection
- `prysm/src/config.rs` - TOML parsing and validated application configuration
- `prysm-core/src/config.rs` - Domain settings, capture resolution and numeric defaults
- `prysm/src/video.rs` - Shared FFmpeg frame stream
- `prysm/src/lib.rs` - Public application entry points
- `prysm/src/input.rs` - Device/file adaptation to a fallible frame stream
- `prysm/src/desktop.rs` - Desktop runtime and threading setup
- `prysm/src/wled.rs` - Headless runtime, WLED streaming, and shutdown
- `prysm/src/stream.rs` - Owned fallible desktop stream consumer and lifecycle tests
- `prysm-capture/src/lib.rs` - PrysmCapturer trait definition
- `prysm-processor/src/nodes/` - Pipeline nodes (band detection, spatial sampling, smoothing)
- `prysm-processor/src/frames/view_frame.rs` - Raw-frame viewport with on-demand pixel decoding
- `prysm-core/src/lib.rs` - Core types and configuration
- `renderers/desktop-renderer/src/lib.rs` - GUI implementation
- `capturers/v4l-capturer/src/capture.rs` - Blocking-to-async bridge pattern
