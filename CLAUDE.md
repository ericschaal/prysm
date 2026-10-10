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

### Workspace Structure

**6 crates, with three application binaries in `prysm`**

Libraries are reusable components for building different binaries:

- Current binaries: `prysm` (camera preview), `video` (video preview), `led` (WLED output)
- Future binaries: LED hardware controllers for different platforms
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
- Slow camera consumers skip intermediate frames; smoothing uses elapsed processing time, while black-bar confirmation still counts delivered frames

### Crate Responsibilities

**Libraries (reusable components):**

- `prysm-core`: Data structures (Color, ColorStrip, EdgeColors, Config) - zero external dependencies
- `prysm-capture`: Frame abstraction, PixelFormat enum, PrysmCapturer trait
- `prysm-processor`: Video analysis → edge color strips (stateful with temporal smoothing)
- `v4l-capturer`: Linux V4L2 video capture implementation
- `desktop-renderer`: egui/eframe GUI for visualization
- `led-renderer`: WLED RGB output over DDP/UDP using `ddp-rs`

**Binaries:**

- `prysm`: Desktop demo/visualizer (V4L capture + desktop renderer)
- `led`: Headless camera or FFmpeg video → processor → WLED via DDP/UDP
- Future: Additional binaries for LED hardware on different platforms

## Common Development Commands

```bash
# Build
cargo build
cargo build --release

# Run main application
cargo run -p prysm

# Play the local Philips HDR test video (requires FFmpeg 9+ on PATH)
cargo run -p prysm --bin video
# Or supply another video path
cargo run -p prysm --bin video -- "/path/to/video.mp4"

# Send edge colors to WLED using led.toml (address, edge counts, source)
cargo run --release -p prysm --bin led
# Or select a different runtime configuration
cargo run --release -p prysm --bin led -- --config /path/to/led.toml

# Run tests
cargo test
cargo test -p prysm-capture  # Run tests in specific crate

# Code quality
cargo check
cargo fmt
cargo clippy
```

## Key Configuration Points

**Desktop visualizer setup (prysm/src/lib.rs):**

- Capture resolution: 640x360 (intentionally low — the camera ISP's hardware downscale integrates every
  source pixel, which is both cheaper and more accurate than sampling a high-res frame)
- Displayed LED count: 300 (desktop renderer default)
- Video device: `/dev/video2`
- Note: This is specific to the desktop visualizer, not a global configuration

`prysm/src/bin/video.rs` is the alternate entry point for video files. It defaults to the local
Philips Ambilight test MP4, plays once at its original rate, and closes at EOF. FFmpeg 9+ decodes
and scales frames to 640x360 sRGB, including HDR tone mapping; audio is not played. Both entry
points share the visualizer pipeline in `prysm/src/lib.rs`. The camera remains the default binary.

`prysm/src/bin/led.rs` reads `led.toml` at startup (or a file selected with
`--config`). It contains `wled_address` with port 4048, a `[leds]` table with
named top/right/bottom/left counts, and a `[source]` table: `type = "camera"`
with an optional `device`, or `type = "video"` with an optional `path`.
Wiring starts at the bottom-left viewed from the front, running up the left edge,
across the top, down the right, and back along the bottom. Video input defaults
to the same Philips clip as the desktop video binary and exits successfully at
EOF. Both use the FFmpeg feed in `prysm/src/video.rs`. Ctrl+C cancels capture;
capture and send failures exit with their original error context. WLED's realtime timeout restores
its normal effect.

The current pipeline assumes sRGB. `LinearColor` uses floating-point math, but input decoding
uses the sRGB transfer function and preview output is 8-bit RGB. BT.2020 YUV matrix support
does not include PQ/HLG transfer decoding or an HDR display path, so file playback tone-maps first.

**Default Config (prysm_core::Config):**

Edit the Rust `Config` passed to `PrysmProcessor::new` (in `prysm/src/lib.rs` for the demo).
Start with `Config::default()` and override only what you want to change:

```rust
let config = prysm_core::Config {
    brightness_percent: 60,
    smoothing_seconds: 0.2,
    sample_density: prysm_core::SampleDensity(150),
    ..prysm_core::Config::default()
};
```

| Setting | Default | What it changes |
| --- | --- | --- |
| `brightness_percent` | 80 | 0 turns lights off; 100 is full brightness. |
| `smoothing_seconds` | 0.1 | Time to complete 95% of a transition, independent of frame rate. Zero disables smoothing. |
| `sample_density` | `SampleDensity(60)` | Color samples per 1000 pixels of cropped edge length, independent of LED count. |
| `edge_depth_percent` | 15 | How far inward to read colors, as a percentage of picture height after cropping. Influence fades quadratically from the edge to zero at the inner boundary. |
| `remove_black_bars` | true | Follow the picture inside stable black bars. Set false to sample the full frame. |

Brightness is clamped to 0–100, edge depth to 1–50. Edge regions never overlap their opposite
edge. Negative and non-finite smoothing durations disable smoothing. Every frame is sampled;
density changes how finely the edge bands are divided, with all region pixels still contributing.
Sample counts are capped at one per pixel, with at least one per edge. Black-bar tuning is internal.
`process_frame` uses monotonic processing time; deterministic replay can use `process_frame_at`
with a shared clock origin plus source timestamps.

The old fractional `brightness` and `edge_depth` fields are replaced by integer percentages;
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

- `PrysmProcessor` chains typed nodes: `BandDetector` (letterbox/pillarbox viewport) → `EdgeSampler`
  (edge-weighted linear-light region averaging on every frame) → `TemporalSmoothing`
- Frames carry their YUYV range and matrix from capture into both processing and preview.
- Brightness scales the final edge colors after smoothing.
- Frames stay in their raw capture format end-to-end; there is no full-frame RGB decode. Each node
  decodes only the pixels it reads (`ViewFrame::average_edge_linear`). Band detection reads luma via
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

- `prysm/src/main.rs` - Camera entry point
- `prysm/src/bin/video.rs` - Desktop video file entry point
- `prysm/src/video.rs` - Shared FFmpeg frame stream
- `prysm/src/lib.rs` - Shared application orchestration and threading setup
- `prysm/src/stream.rs` - Owned fallible desktop stream consumer and lifecycle tests
- `prysm-capture/src/lib.rs` - PrysmCapturer trait definition
- `prysm-processor/src/nodes/` - Pipeline nodes (band detection, edge sampling, smoothing)
- `prysm-processor/src/frames/view_frame.rs` - Raw-frame viewport with on-demand pixel decoding
- `prysm-core/src/lib.rs` - Core types and configuration
- `renderers/desktop-renderer/src/lib.rs` - GUI implementation
- `capturers/v4l-capturer/src/capture.rs` - Blocking-to-async bridge pattern
