# Architecture

Prysm runs on a Linux or macOS host. It captures or receives video, computes
colors along the picture edges, and sends those colors to WLED. The browser
panel is part of the same application.

```text
HDMI capture card → V4L2 / AVFoundation capturer       ─┐
Rooted LG TV → webOS capturer → WebSocket ───────────────┼→ processor → WLED → LEDs
Video file → FFmpeg ────────────────────────────────────┘      ↕
                                                     browser panel
```

A **capture device** supplies the video: an HDMI capture card, or the webOS capturer
running on a rooted LG TV. A file is also supported for testing. The **Prysm
application** owns host capture/receiving, processing and controls. The **WLED
renderer** sends colors to the lights; it is the only supported LED renderer.
The webOS capturer still needs a separate Prysm host.

## Ownership and boundaries

[`application.rs`](../../apps/prysm/src/application.rs) composes HTTP, the
controller, preview and the webOS receiver. Startup stays idle, including after
setup. HTTP remains available after EOF or capture/output failure.

| Owner | Job |
| --- | --- |
| Settings store | Validate one browser-saved JSON document, check revisions and write atomically. Publish new settings only after persistence succeeds. |
| Controller | Serialize accepted settings and power/source operations, observe WLED, retain operation results and own calibration sessions. Accepted work survives a disconnected caller. |
| Pipeline | Own one input, one processor and one WLED output through Idle, Starting, Running, Stopping, Completed and Failed states. |
| Processing worker | Poll input and process frames sequentially on a dedicated OS thread with a current-thread Tokio runtime. Apply processing changes between frames. |
| WLED task | Consume the latest edge colors and send nonblocking DDP packets. It runs on the application's current-thread Tokio runtime alongside HTTP. |

Process options such as bindings, allowed hosts and the storage path come from
CLI/environment. They are separate from browser settings; they do not override
fields in the JSON document. Input/output edits recreate a running pipeline;
processing edits apply live. See [settings ownership](decisions/2026-11-03-settings-ownership.md)
and [HTTP contracts](control-api.md).

WLED state comes from its WebSocket with HTTP resynchronization. Switching uses
direct HTTP observations to confirm the result. Observations from a retired
configuration cannot replace current state. The renderer only sends colors;
WLED power/effects control belongs to the application.

## Processing and color

Capture publishes immutable `Frame` values. Each producer has a `FramePool` with
up to four retained buffers; readers share pixels. A buffer is reused only when
all readers release it. If every slot is held, the producer replaces a pool
reference rather than waiting. The pool limit does not include displaced buffers
still held by readers.

`PrysmProcessor` runs black-bar detection, spatial sampling and temporal smoothing,
then shadow lift, calibration correction and brightness. A `ProcessedSnapshot`
pairs source pixels, source-pixel crop and edge colors from the same pass.
Lighting receives only `EdgeColors`; it never retains source pixels.

Frame delivery favors the latest result instead of accumulating a backlog.
Lifecycle commands remain ordered and reliable. Each run gets a fresh output
channel so a restart cannot send an old run's colors.

Core holds shared image/color concepts and their invariants. It has a `yuv`
conversion dependency, but no serialization, HTTP or runtime dependencies.
`prysm-contract` adds disk/HTTP validation and schema ownership. Similar types at
these boundaries can have different rules: core clamps some controls, while
contract values reject invalid user input. Keep platform handles and wire-format
choices with their adapters.

YUYV frames carry range/matrix metadata and use a reusable RGB conversion buffer
for sampling. Processing assumes sRGB transfer: a BT.2020 matrix does not decode
PQ/HLG. File input tone-maps first. Use `process_frame_at` with consistent source
timestamps for deterministic replay. For the black-bar algorithm and evidence,
see the [detector investigation](../../research/black-bar-detection-2026-11-02/README.md).

## Preview observation

Preview observes the pipeline; opening it never starts capture. It retains the
last paired snapshot across stop/EOF and reconnects when a new run starts.
A lazy JPEG worker allows one active and one queued request. It encodes the
freshest snapshot, fits it within 480×480 and caches by publication ID.

The same-origin `/api/preview` WebSocket allows four viewers. Each requests
`next` after display, at up to 8 fps. Hidden views stop requesting. Slow or broken
viewers affect preview only. Crop coordinates refer to source pixels; horizontal
edge arrays run left-to-right and vertical arrays top-to-bottom. See the
[preview protocol](control-api.md#browser-preview-and-live-settings).

## Shutdown ordering

Stop cancels capture, finishes accepted processing, closes publication and lets
the output consume its final unseen result. It then observes native/FFmpeg cleanup
and joins the processing worker. Only after that may Off/effects commands change
WLED state; a late DDP packet must never undo them.

A five-second cleanup deadline reports failure, not successful cancellation.
Timed-out runs retain their handles and cannot restart until cleanup really
finishes. Output failures cancel capture and retain the cause. Application exit
drains accepted control work, joins the pipeline and receiver, then shuts down
preview and HTTP. Preview is outside the lighting stop barrier.

## Workspace map

| Location | Responsibility |
| --- | --- |
| `apps/prysm` | Application composition, settings, controller, pipeline, preview and HTTP policy |
| `apps/prysm-webos-capture` | webOS capturer: video capture, encoding and transport |
| `apps/prysm-webos-service` | TV Luna supervisor, permission setup and worker lifecycle |
| `crates/capture` | V4L2 capturer (Linux), AVFoundation capturer (macOS), webOS receiver and protocol |
| `crates/processing` | Black-bar detection, sampling and smoothing |
| `crates/rendering` | WLED DDP renderer |
| `crates/common` | Core data, validated contracts, HTTP adapter and metrics |
| `crates/web` | Embedded host panel, TV frontend and calibration patterns |

Backend libraries may create additional threads. This describes ownership, not an
exact thread count. For performance checks and measurement limits, see [debugging](debugging.md).
