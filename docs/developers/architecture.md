# Architecture

`application` is the composition root. Startup stays idle. The main OS thread runs
one Tokio current-thread runtime for HTTP, the application controller, pipeline
lifecycle, asynchronous WLED HTTP, and the tracked WLED DDP consumer. HTTP remains
available after EOF or capture/output failure. No multithread Tokio runtime is used.

## Ownership and boundaries

The controller owns saved configuration, requested source, operation IDs and
revision state. Bounded reliable commands with oneshot replies connect the HTTP
adapter to it; `prysm-api` has no Actix dependency. Accepted mutations continue
when their callers disconnect. The controller retains startup/application,
persistence and WLED work while it observes status and handles shutdown. Competing
mutations conflict. Atomic file writes finish before saved configuration is published;
saved and processor-applied revisions remain distinct. WLED switching preserves
its compensation/rollback and observation checks.

A pipeline lifecycle owner holds the configured input and explicit Idle, Starting,
Running, Stopping, Completed and Failed states. Active resources are attached to
states until their terminal result and joins are observed. Every run has a fresh
latest-result watch channel, so a restarted output cannot transmit a retired frame.
Configuration is applied between frames on the processing owner; input/output
changes recreate a run. Web listen/access-policy changes require application restart.

Each active run owns a dedicated OS thread with a Tokio current-thread runtime.
It polls the selected fallible input stream, runs one sequential `PrysmProcessor`,
and publishes `Arc<ProcessedSnapshot>` containing source Frame, optional source-pixel
Viewport, and EdgeColors from the same pass. Frame pixels and color strips are
already shared immutable buffers. Core data and processing do not know about HTTP,
preview wire formats, runtime workers or application commands. There is one
capturer, one processor and one configured lighting output; no processing graph.

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
The consumer clones the latest snapshot, releases watch borrows, resamples/packs
and sends nonblocking UDP. `WouldBlock` remains a rendering error. This task shares
scheduling with HTTP; it does not provide strict HTTP isolation.

## Preview observation

The application owns a small latest-snapshot bridge that retains the final image
across stop/EOF and reconnects to publications from a new run. It is separate from
the per-run output channel. Opening preview never starts capture.

The encoder is one synchronous OS worker, started on first demand, with a bounded
channel allowing one active and one queued request. It reads the freshest paired
snapshot when work starts, downscales to at most 480px on either axis, encodes JPEG,
and reuses an encoded response for the same publication. No encoder runs without
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
of LED count. Black-bar confirmation uses elapsed time and retains confirmed crop through
dark scenes. Live configure preserves temporal history except history specifically
invalidated by the changed setting.

Frames carry YUYV range/matrix metadata. Processing assumes sRGB transfer; BT.2020
matrix support does not decode PQ/HLG. File input tone-maps first. Use `process_frame_at`
with shared clock origin/source timestamps for deterministic replay; `process_frame`
uses monotonic processing time. See [color measurements](../../research/webos-capture/2026-11-01-hdr-brightness/README.md)
and [webOS ownership](webos.md) for backend-specific constraints.
