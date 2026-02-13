# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Prysm** is an ambient lighting system (ambilight/bias lighting) that captures video from a camera, analyzes edge colors, and generates color gradients for LED strips.

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
                          EdgeSpectrums → Renderer (LED strips)
```

**Core Abstractions:**
- `PrysmCapturer` trait: Extensible video capture interface
- `StreamWatcher` pattern: Bridges async streams to sync watch channels
- `stream_split()`: Broadcast channel for multi-consumer streams

### Crate Responsibilities

**Libraries (reusable components):**
- `prysm-core`: Data structures (Color, ColorSpectrum, EdgeSpectrums, Config) - zero external dependencies
- `prysm-capture`: Frame abstraction, PixelFormat enum, PrysmCapturer trait
- `prysm-processor`: Video analysis → edge color spectrums (stateful with temporal smoothing)
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

# Run tests
cargo test
cargo test -p prysm-capture  # Run tests in specific crate

# Code quality
cargo check
cargo fmt
cargo clippy
```

## Key Configuration Points

**Desktop demo binary (prysm/src/main.rs):**
- Capture resolution: 1920x1080
- LED count: 40
- Video device: `/dev/video2`
- Note: This is specific to the desktop visualizer, not a global configuration

**Default Config (prysm_core::Config):**
- Target FPS: 30
- Brightness: 0.8
- Temporal smoothing: 0.7
- Edge sampling: 50 samples per 1000px
- Edge depth: 100px

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
- `ColorProcessor` is stateful (maintains smoothing history)
- Supports YUYV and RGB24 with format-specific optimizations
- MJPEG and BGR24 currently return black spectrums

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

- `prysm/src/main.rs` - Application orchestration and threading setup
- `prysm/src/stream.rs` - StreamWatcher and stream_split patterns
- `prysm-capture/src/lib.rs` - PrysmCapturer trait definition
- `prysm-processor/src/color.rs` - Edge color sampling logic
- `prysm-core/src/lib.rs` - Core types and configuration
- `renderers/desktop-renderer/src/lib.rs` - GUI implementation
- `capturers/v4l-capturer/src/capture.rs` - Blocking-to-async bridge pattern
