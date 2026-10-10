# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Prysm** is an ambient lighting system (ambilight/bias lighting) that captures video from a camera, analyzes edge
colors, and generates color gradients for LED strips.

**Purpose:** Library ecosystem for capturing video, analyzing edge colors, and driving LED output.

**Current State:**

- Desktop demo/visualizer binary (Linux/V4L) is working
- Library components are modular and reusable
- Future: Additional binaries for LED hardware control on different platforms
- Architecture supports multiple capture sources and rendering backends

**Long-term Goal:** Move toward `no_std` compatibility for embedded/microcontroller targets

- Currently uses std library (tokio, async I/O)
- Keep this goal in mind when adding dependencies or features
- `prysm-core` already has zero external dependencies as a step toward this goal

## Architecture Overview

### Workspace Structure

**7 crates: 6 libraries + 1 desktop demo binary**

Libraries are reusable components for building different binaries:

- Current binary (`prysm`): Desktop visualizer demo (Linux/V4L + egui GUI)
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
V4lCapturer → Frame Stream → split (broadcast)
                               ↓           ↓
                          PrysmProcessor  Renderer (video display)
                               ↓
                          EdgeColors → Renderer (LED strips)
```

**Core Abstractions:**

- `PrysmCapturer` trait: Extensible video capture interface
- `StreamWatcher` pattern: Bridges async streams to sync watch channels
- `stream_split()`: Broadcast channel for multi-consumer streams

### Crate Responsibilities

**Libraries (reusable components):**

- `prysm-core`: Data structures (Color, ColorStrip, EdgeColors, Config) - zero external dependencies
- `prysm-capture`: Frame abstraction, PixelFormat enum, PrysmCapturer trait
- `prysm-processor`: Video analysis → edge color strips (stateful with temporal smoothing)
- `v4l-capturer`: Linux V4L2 video capture implementation
- `desktop-renderer`: egui/eframe GUI for visualization
- `led-renderer`: Stub for future hardware LED driver

**Binaries:**

- `prysm`: Desktop demo/visualizer (V4L capture + desktop renderer)
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
- Return a stream of `Frame` objects
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
- `prysm/src/bin/video.rs` - Video file entry point and FFmpeg frame stream
- `prysm/src/lib.rs` - Shared application orchestration and threading setup
- `prysm/src/stream.rs` - StreamWatcher and stream_split patterns
- `prysm-capture/src/lib.rs` - PrysmCapturer trait definition
- `prysm-processor/src/nodes/` - Pipeline nodes (band detection, edge sampling, smoothing)
- `prysm-processor/src/frames/view_frame.rs` - Raw-frame viewport with on-demand pixel decoding
- `prysm-core/src/lib.rs` - Core types and configuration
- `renderers/desktop-renderer/src/lib.rs` - GUI implementation
- `capturers/v4l-capturer/src/capture.rs` - Blocking-to-async bridge pattern
