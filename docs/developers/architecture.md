# Architecture

`application` is the composition root. Startup stays idle. The main OS thread runs
one Tokio current-thread runtime for HTTP, the application controller, pipeline
lifecycle, asynchronous WLED HTTP, and the tracked WLED DDP consumer. HTTP remains
available after EOF or capture/output failure. No multithread Tokio runtime is used.

## Ownership and boundaries

The controller owns saved configuration, requested source, operation IDs and
revision state. Bounded reliable commands with oneshot replies connect the HTTP
adapter in `prysm-api` to it. Accepted mutations continue
when their callers disconnect. The controller retains startup/application,
persistence and WLED work while it observes status and handles shutdown. Competing
mutations conflict. Atomic file writes finish before saved configuration is published;
saved and processor-applied revisions remain distinct. WLED switching preserves
its compensation/rollback and observation checks.

WLED observations use one `/ws` connection for pushed state/info snapshots, with
an immediate HTTP `/json/si` read and a 30-second HTTP resynchronization interval.
The socket sends WLED's `p`/`pong` heartbeat every 15 seconds and reconnects after
30 seconds without a reply. Failed connections retry with exponential delays from
one to 30 seconds; a valid snapshot resets the delay. HTTP observation continues
while WebSocket is unavailable. Delayed HTTP responses cannot replace a newer
pushed snapshot. Configuration changes retire the previous observation session;
revision checks reject retired updates. The controller owns and cancels the
observer future on shutdown, while draining accepted operations as before. Local
pipeline status refreshes every second without a WLED request. Switching still
uses direct HTTP observations to confirm commands and preserve rollback behavior.

A pipeline lifecycle owner holds the configured input and explicit Idle, Starting,
Running, Stopping, Completed and Failed states. Active resources are attached to
states until their terminal result and joins are observed. Every run has a fresh
latest-result watch channel, so a restarted output cannot transmit a retired frame.
Configuration is applied between frames on the processing owner; input/output
changes recreate a run. Web listen/access-policy changes require application restart.

Each active run owns a dedicated OS thread with a Tokio current-thread runtime.
It polls the selected fallible input stream, runs one sequential `PrysmProcessor`,
and publishes only `EdgeColors` to lighting. Preview receives a `ProcessedSnapshot`
containing source Frame, optional source-pixel Viewport, and EdgeColors from the
same pass. Cloning these values shares their immutable pixel/color buffers without
allocating wrappers. Core data and processing do not know about HTTP,
preview wire formats, runtime workers or application commands. There is one
capturer, one processor and one configured lighting output; no processing graph.

Each native capture session, webOS connection and file reader owns a `FramePool`
with at most four retained pixel buffers. It acquires a fixed-length writable
buffer, fills it directly, then publishes an immutable Frame. `Arc::get_mut`
allows reuse only after every consumer has released that buffer. There are no
return queues or custom destructors. If all four buffers are held, capture replaces
one pool reference with a fresh allocation; existing readers retain their old
pixels and capture never waits for them. The cap bounds retained pool entries,
not buffers still owned by consumers. Four 1080p RGB buffers occupy about 25 MB;
capacities grow as needed. Ending the producer releases its pool references;
observers may still retain the final frame.

Steady-state reuse avoids pixel allocation and clearing, not the copy from native
capture storage or required color conversion. `FrameWriter` keeps validated
geometry separate from fixed-length mutable pixels; reads/conversions must finish
successfully before publication. Cancelling a partial read publishes nothing.
Lighting never retains source pixels. Preview retains its latest snapshot and any
currently encoding snapshot, but its encoded-response cache stores only the
publication ID and encoded response.

The processing loop retains its in-flight stream read across configuration events,
services cancellation/configuration/availability, and yields even for continuously
ready input. Native completion monitors keep running while device resources are
being released. V4L blocking capture, AVFoundation callback queues, webOS receiver/
decode and FFmpeg remain backend-specific work. File cleanup explicitly terminates
and reaps FFmpeg. The webOS listener belongs to the configured input, survives run
stop, and is shut down and joined on reconfiguration or application exit.

WLED implements the single tagged production Output variant. Its DDP adapter stays
in `prysm-renderer-wled`; WLED HTTP power/effect capabilities stay in the application
backend rather than a universal renderer interface. Hostname resolution occurs on
the processing/setup worker before the renderer moves into the application task.
The consumer clones the latest edge colors, releases watch borrows, resamples/packs
and sends nonblocking UDP. `WouldBlock` remains a rendering error. This task shares
scheduling with HTTP; it does not provide strict HTTP isolation.

## Type ownership

Shared image and processing concepts belong in `prysm-core`: frames and pixel
metadata, capture resolution, viewport/edges, color strips, processing settings,
and paired processing snapshots. Core has an existing `yuv` dependency for color
conversion; it does not depend on serialization, HTTP, the runtime, or webOS.
Move a type here when its meaning and invariants are shared, not just its name or
field layout.

The November 2026 type audit covered the maintained applications and capture,
processing, rendering, contract/API, metrics, and frontend code. These boundaries
deliberately retain distinct types:

| Similar types | Why they remain separate |
| --- | --- |
| Core `Config`/setting scalars and contract `ProcessingSettings`/scalars | Core clamps percentages and disables invalid smoothing; disk/HTTP settings reject invalid input. Sample density uses `usize` in core and `u32` on the wire. Contract defaults already come from core. |
| Core and contract `CaptureResolution` | The contract wrapper delegates validation/defaults to core and adds serialization/schema ownership. webOS additionally restricts dimensions to even values up to 1080p. |
| Core, webOS, and metrics pixel formats | Core describes processing storage and YUYV color metadata; the wire has fixed RGB/NV12/NV16 IDs; metrics combine formats into labels. A merged enum would admit unsupported states. |
| webOS `Compression` and `Codec` | Compression selects a configured encoder backend; codec identifies the actual payload. Metrics codec labels also include an unspecified value. |
| Application and webOS `Source` | Prysm/WLED lighting selection and video/blended capture selection are different decisions. |
| `Config`, `PipelineConfig`, and runtime state | Saved application configuration, a pipeline configuration subset, and owned running resources have different lifetimes and responsibilities. `PipelinePhase` is an observation of runtime state. |
| Contract `LedCounts` and renderer counts | Configuration requires a positive total; the renderer permits empty output and enforces the WLED limit at its boundary. |
| `Frame`, `FrameView`, `ProcessedSnapshot`, and private `FrameLayout` | Immutable pixels, processor sampling/cropping access, a published result, and a layout-change cache key serve different purposes. |
| `CapturedFrame` and `EncodedFrame` | Borrowed raw capture pixels and encoded transport bytes mark distinct pipeline stages despite matching fields. |
| `Color`, `LinearColor`, and renderer `Rgb8Payload` | sRGB values, linear-light arithmetic, and packed DDP bytes must retain their color/encoding semantics. |
| `ApiError` and `ApiFailure` | Serialized error data belongs to the contract; HTTP status and response behavior belong to the API adapter. |

Keep platform handles, worker commands, WLED response projections, and metrics
labels with their adapters. REST frontend models derive from OpenAPI; the webOS
frontend describes the separate Luna interface.

Within core, `frame/mod.rs` owns immutable frames and format/geometry validation;
`frame/pool.rs` owns reusable producer storage and its exclusive writer. Keeping
the writer with the pool preserves private frame construction without exposing an
unchecked constructor. Capture resolution has its own module, while processing
settings live together in `config.rs`. The webOS protocol separates bounded JSON
control messages in `control.rs` from binary headers and payload compression in
`frame.rs`. Crate-root exports keep callers independent of these file boundaries.

## Preview observation

The application owns a small latest-snapshot bridge that retains the final image
across stop/EOF and reconnects to publications from a new run. It is separate from
the per-run output channel. Opening preview never starts capture.

The encoder is one synchronous OS worker, started on first demand, with a bounded
channel allowing one active and one queued request. It reads the freshest paired
snapshot when work starts, downscales to at most 480px on either axis, encodes JPEG,
and reuses an encoded response for the same publication ID. No encoder runs without
demand. At most four WebSocket viewers may connect; each requests the next response
after decode/display, with at most 8 fps. Hidden/closed views stop requesting. Slow,
disconnected and malformed clients affect only preview. Image/crop/edge drawing
stays outside React state. Crop coordinates always refer to source pixels; color
arrays use screen coordinates (horizontal left-to-right, vertical top-to-bottom).

`/api/preview` is inside the same host/origin/security policy as REST. It accepts
only `next`, caps inbound messages and connections, and bounds send duration. JSON
responses pair a frame identifier and applied revision with JPEG/base64, source and
preview dimensions, crop and sRGB edge strips. Idle/busy/error replies are local to
preview. CSP permits only same-origin resources and data images for JPEG decoding.

## Shutdown ordering

Stop cancels capture, finishes accepted synchronous processing, ceases publication,
closes the per-run result sender, and lets WLED consume its final unseen result.
The session observes output completion, native release/FFmpeg cleanup, and processing
OS-thread join before acknowledging stop. Only then may switching send final WLED
Off/effects commands. Output failure cancels capture and retains its error; a failed
output need not send again. Preview clients/encoding are outside the lighting stop
barrier and are cancelled/joined separately at application shutdown.

A five-second cleanup deadline reports failure; it does not prove work stopped.
Timed-out runs retain their handles and remain unavailable for restart until actual
cleanup completes. No successful stop or final WLED command is permitted while
output could still send. Application shutdown drains accepted control work, shuts
down/join the pipeline and webOS listener, then shuts down preview and HTTP.

## Measurements and platforms

On the development Mac, 100 full WLED resample/pack/loopback UDP calls measured:
300 LEDs p50 10.9µs, p99 66.5µs, max 94.5µs; maximum 65535 LEDs p50 1.75ms,
p99 1.92ms, max 1.95ms. This fits the chosen 5ms local handler budget; repeat with
`cargo test -p prysm --lib measure_synchronous_wled_handler -- --ignored --nocapture`.
Re-evaluate on deployment hardware before assuming the same control budget; move
the whole consumer to one owned worker if measurements exceed it. These timings
include synchronous packing/socket acceptance, not physical screen-to-light latency.
Backend/native libraries may create auxiliary threads; this documents owned contexts,
not an exact process thread count. Linux and physical device behavior require target
CI/hardware checks; macOS results do not establish those properties.

`cargo run --release -p prysm-processor --example frame_recycling` compares fresh
and pooled input buffers while retaining the previous frame, then repeats with
processing enabled. It asserts zero steady-state capture allocations and matching
output checksums. On the development Mac, three runs at 1080p RGB/density 128
reduced copy-plus-processing allocations from 18 to 16 per frame and allocated
bytes from 6,239,544 to 18,704. Median time was 6.006ms versus 5.986ms: no convincing
CPU gain. This is an allocation-volume improvement; pixel copying, processing
allocations and decoder-internal allocations remain. The benchmark excludes device
I/O, preview encoding and publication wrappers (which no longer allocate).

## Workspace map

| Location | Responsibility |
| --- | --- |
| `apps/prysm` | Composition, control/persistence, pipeline workers, preview and HTTP policy |
| `apps/prysm-webos-capture` | TV capture/encode/transport worker |
| `apps/prysm-webos-service` | Native Luna supervisor, setup and worker lifecycle |
| `crates/capture` | Native capture, receiver and webOS protocol |
| `crates/processing` | Black-bar detection, spatial sampling, temporal smoothing |
| `crates/rendering` | Framework-independent WLED DDP adapter |
| `crates/common` | Core immutable data, validated contract, typed HTTP adapter and metrics |
| `crates/web` | Embedded React UI/assets and separate webOS frontend |

## Processing and color

`PrysmProcessor` chains BlackBarDetector → SpatialSampler → TemporalSmoother.
Sampling averages picture areas and projects colors onto the perimeter. Shadow lift
follows smoothing; brightness scales final colors. Density controls sampling independently
of LED count. Black-bar detection tracks the four picture boundaries independently,
combining distributed observations of a dark exterior and sustained picture interior.
It rejects small overlays and retains confirmed coordinates through dark or ambiguous
scenes. Fresh consistent observations confirm deeper cropping in about 1.5 seconds
and restore visible picture in about 0.4 seconds; capture gaps do not count as evidence.
Live configure preserves temporal history except history specifically invalidated by
the changed setting. See the [detector investigation](../../research/black-bar-detection-2026-11-02/README.md)
for the motivating failures, design and validation limits.

Black-bar detection reads the original frame. For sampling, YUYV is converted
with the existing SIMD converter into one reusable RGB buffer, using its range
and matrix metadata; RGB/BGR inputs need no conversion buffer. Shadow lift and
brightness share one sample pass after smoothing, with copy-on-write preserving
the unadjusted history and published snapshots. Identity adjustments share colors.

Frames carry YUYV range/matrix metadata. Processing assumes sRGB transfer; BT.2020
matrix support does not decode PQ/HLG. File input tone-maps first. Use `process_frame_at`
with shared clock origin/source timestamps for deterministic replay; `process_frame`
uses monotonic processing time. See [color measurements](../../research/webos-capture/2026-11-01-hdr-brightness/README.md)
and [webOS ownership](webos.md) for backend-specific constraints.
