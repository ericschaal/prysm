# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Prysm** is an ambient lighting system (ambilight/bias lighting) that captures video from a camera, analyzes edge
colors, and generates color gradients for LED strips.

**Purpose:** Library ecosystem for capturing video, analyzing edge colors, and driving LED output.

**Current State:**

- One HTTP controller executable runs on macOS and Linux with an actor pipeline
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

**Rust workspace with application binaries and a standalone webOS capture CLI**

Applications live in `apps/`. Reusable crates are grouped under
`crates/{capture,processing,rendering,common}/`; the standalone Rust webOS sender
and its shared wire protocol live beside the capture crate. See the workspace table in README.md.

Libraries are reusable components for capture, processing and rendering:

- `prysm` serves HTTP and wires device/file capture, processing, and WLED actors
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
- `prysm-renderer-desktop`: egui/eframe GUI for visualization
- `prysm-renderer-wled`: WLED RGB output over DDP/UDP using `ddp-rs`

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

**Application setup (`apps/prysm/src/main.rs`, `apps/prysm/src/control`, `apps/prysm/src/actors`):**

The `prysm` engine reads `prysm.toml` (or `--config PATH`). Prysm
owns this one file and atomically saves UI changes into it. Validated application
configuration lives in `prysm-contract`: REST policy, optional WLED HTTP endpoint, input,
capture resolution, DDP address/layout/rate, processing settings, source, revision.
No separate preferences file or legacy configuration migration is supported.

The main Actix runtime serves HTTP/control. A lifecycle actor serializes start,
stop, and configuration commands. Each active pipeline runs capture, processing,
and rendering actors on a dedicated Arbiter. Typed recipients carry best-effort
data through bounded mailboxes. Processing changes use `ConfigureProcessor`;
input/resolution/output changes recreate the session. Stop capture, drain accepted
output, then join the thread before applying WLED Off/effects commands.

`[input]` selects `device` plus a Linux path/macOS AVFoundation UID, `file` plus
a path, or `webos` plus a TCP listen address. File input uses FFmpeg 9+ and completes at EOF without stopping HTTP.
`[output]` selects `type = "wled"` with a DDP address, rate and LED layout, or
`type = "egui"` with an egui refresh rate. Counts total 1–65535; wiring runs clockwise from bottom-left.
`--features desktop` enables native egui output in the same executable. It runs
on the OS main thread with HTTP, the controller, and actors in a background runtime
and needs no WLED. Both outputs start idle and use the same REST commands.
`prysm-egui.toml` documents file input; EOF keeps the final frame visible.
Startup never starts capture or restores saved On intent.

The current pipeline assumes sRGB. `LinearColor` uses floating-point math, but input decoding
uses the sRGB transfer function and preview output is 8-bit RGB. BT.2020 YUV matrix support
does not include PQ/HLG transfer decoding or an HDR display path, so file playback tone-maps first.

**Default Config (prysm_core::Config):**

`prysm-contract::ProcessingSettings` converts to `prysm_core::Config` at actor
creation or explicit configuration messages. Core config contains processing
settings only. Capture resolution is validated separately. Distinct newtypes
prevent mixing brightness, spatial reach, smoothing duration, and resolution.
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
| `shadow_lift_percent` | 0 | 0–100 strength of a black-preserving linear-light shadow boost. |
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
exclude the other bars when inspecting narrow content. Cropping requires both opposing bars;
if either is absent, that axis returns to the full picture after confirmation. Opposing margins differing by more than
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

- YUYV color conversion tests in `crates/capture/prysm-capture/src/yuyv.rs`
- Test black, white, gray, and color-tinted pixel conversions
- Verify output dimensions

## Important Implementation Details

### When adding new capture sources:

- Implement the `PrysmCapturer` trait
- Return a stream of `Result<Frame>` objects; preserve startup/runtime failure context
- Use the shared latest-frame capture channel; keep terminal results separate from replaceable frames
- Handle blocking I/O by spawning OS threads (see the prysm-capture Linux adapter)
- Use Arc-wrapped frame data for zero-copy sharing

### When working with the processor:

- `PrysmProcessor` chains typed nodes: `BlackBarDetector` (letterbox/pillarbox viewport) → `SpatialSampler`
  (whole-picture area averaging and distance-weighted projection onto the perimeter) → `TemporalSmoother`
- Frames carry their YUYV range and matrix from capture into both processing and preview.
- Shadow lift follows smoothing, then brightness scales the final edge colors.
- The controller owns installation TOML and saved JSON preferences, and runs capture as a supervised task.
- `prysm-contract` owns validated config/HTTP types and OpenAPI schemas; core stays dependency-free.
- The same application runs on macOS and Linux; there is no IPC or process-manager abstraction.

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

- `apps/prysm/src/main.rs` — CLI and one application entrypoint
- `apps/prysm/src/application.rs` — common controller startup and optional native window
- `apps/prysm/src/control/mod.rs` — HTTP server startup and shutdown
- `apps/prysm/src/control/http.rs` — panel routes, assets, and host/origin policy
- `apps/prysm/src/control/controller.rs` — serialized configuration/source mutations
- `apps/prysm/src/control/persistence.rs` — atomic TOML persistence
- `apps/prysm/src/control/switching.rs` — WLED transition ordering and compensation
- `apps/prysm/src/actors/runtime.rs` — session lifecycle actor and status
- `apps/prysm/src/actors/pipeline.rs` — thread setup, wiring, drain, and join
- `apps/prysm/src/actors/{capturer,processor,wled,egui}.rs` — typed data actors
- `apps/prysm/src/input.rs`, `video.rs` — fallible device/file frame streams
- `crates/common/prysm-api/src/lib.rs` — actual HTTP handlers, route registration, and generated OpenAPI
- `crates/common/prysm-contract/src/{config,resources,processing}.rs` — validated configuration
- `crates/common/prysm-contract/src/{state,identifiers}.rs` — shared HTTP DTOs and identifiers
- `crates/capture/prysm-capture/src/lib.rs` — capture trait and frame types
- `crates/processing/prysm-processor/src/nodes/` — detection, sampling, smoothing
- `crates/common/prysm-core/src/config.rs` — core processing settings and defaults

## Controller architecture and development

See [actor runtime](docs/actor-runtime.md) and [web UI guide](docs/control-panel.md).
Keep lifecycle commands separate from best-effort frame traffic. Do not introduce
worker binaries, private IPC, or service-manager APIs. `AtomicResponse` is used
only to serialize lifecycle commands across awaits; frame handlers are synchronous.

Run Rust with `cargo run -p prysm -- --config prysm.toml`. Run Vite independently
from `apps/prysm-web/control-ui` with `npm run dev`. `cargo run -p prysm-api --bin export-openapi` prints OpenAPI from
`prysm-api::router()`; `npm run generate-api` captures that output and generates
the TypeScript client. Generation stays separate from frontend builds to avoid
invoking Cargo recursively from `prysm-web/build.rs`.
openapi-typescript/openapi-fetch supply the TypeScript HTTP client. The independent `prysm-web` binary forwards REST calls to Prysm; it owns no
configuration or pipeline. Its build script installs locked npm dependencies and
builds the Vite assets, which are always embedded.
Run `cargo run -p prysm-web`, then open port 8788 (Prysm REST stays on 8787).
Building `prysm-web` or the workspace requires Node.js 24 and npm; building
`prysm` alone requires neither.

Debian seeds `/etc/prysm/prysm.toml` from a packaged template and runs
as the `prysm` service user. The writable runtime file is authoritative.
Preserve WLED ordering, post-stop Off, compensation, and clean shutdown tests.
