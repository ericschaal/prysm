# Pipeline migration plan and verification

Scope: one session, one configured WLED output, optional browser observation. Preserve
WLED settings, revision/conflict/persistence/rollback semantics and native release barriers.
Unrelated untracked research Python cache is outside this change.

## Contracts and ownership

- Shared core `ProcessedSnapshot { frame: Frame, viewport: Option<Viewport>, colors: EdgeColors }`.
  Fan-out uses `Arc<ProcessedSnapshot>`; Frame already shares immutable pixels.
- Preview: same-origin `/api/preview` WebSocket, client sends text `next`, one outstanding
  request per connection. Server replies with one JSON message: `{type:"frame", frame_id,
  revision, source_width, source_height, width, height, viewport, colors, jpeg}`.
  `viewport` is null or `{x,y,width,height}` in SOURCE pixels; `colors` has top/right/bottom/left
  arrays of sRGB `[r,g,b]` bytes in core edge order; `jpeg` is base64 JPEG.
  Non-frame reply `{type:"idle"}` (no snapshot) or `{type:"busy"}` (bounded worker full).
  Local errors `{type:"error",message}`. Begin at 480px wide, at most 8 fps/viewer,
  maximum 4 connections, one encoder active plus one queued request globally.
  No encoding until requested; retain last snapshot independently of per-run output channel.
- Effective settings/revision come from GET/PUT /api/settings; PUT replaces settings with expected_revision.
  Saving is distinct from operation completion and pipeline.applied_revision. Settings edits
  the full application model; external changes require explicit review/reload. CLI/environment
  configure process options separately; they are never browser settings.
- Processing owner applies settings between frames. Stop cancels capture, completes accepted
  processing, closes publication, drains final unseen output, observes native release and joins
  processing thread before final WLED command. Preview is outside this stop barrier.

## Work partition and integration

Lead owns core snapshot, shared contract/generated API/manifests, application composition,
controller/API migration, browser preview integration, documentation and combined verification.
Backend agent owns apps/prysm/src/pipeline/, new apps/prysm/src/preview.rs, input.rs/video.rs
and targeted backend tests. Coordinate composition and manifests through lead.
Web agent owns control-ui source (except generated OpenAPI/types) and web UI tests.
No concurrent lockfile/generated changes. Agents run targeted checks; lead runs combined builds.

Order: baseline → shared snapshot → real live-settings UI → integrated bounded preview →
verify parity → integrate browser preview → pipeline workers → controller commands → final verification.
Existing API contracts remain authoritative during transition. Every milestone must build.

## Baseline inventory

The baseline used capacity-one processing queues that dropped incoming frames,
serialized accepted configuration writes, and waited for native completion during stop.
The webOS receiver listener belongs to InputSource and survives run stop.
File stream uses FFmpeg rawvideo; partial-read cancellation requires particular care.
Hardware and physical screen-to-light measurements are unavailable unless explicitly recorded.

## Baseline observations (local macOS, 2026-11-02)

Pre-migration host library suite: 30 passed, 1 FFmpeg-dependent ignored. Restricted sandbox
initially denied socket creation; rerunning the same compiled baseline test binary with
loopback socket access passed. Release `prysm-processor --example bench`, default settings,
120 prerecorded synthetic YUYV frames: 640×360 changing 1.266ms/frame; static 1.157ms;
1920×1080 changing 9.568ms; full decode reference 0.373ms. Boundary is synchronous processor
call wall time, excluding capture, queues, network, JPEG/browser and physical LED response.
No native camera/TV/WLED hardware or physical end-to-end latency was measured.


## Delivered milestones

| Milestone | Result |
| --- | --- |
| 1. Baseline/inventory | Owned contexts and release barriers recorded above; repeatable processor and output measurements below |
| 2. Paired result | Core ProcessedSnapshot, shared through Arc; source pixels/crop/colors from one processor pass |
| 3. Web application | Overview and Preview & tuning, all six live controls, one saved config/draft transaction, explicit conflict review/reload, accessible responsive controls |
| 4. Browser preview | Bounded lazy JPEG worker, same-policy WebSocket, one outstanding request/viewer, four-viewer cap, hidden/closed teardown, final-image retention |
| 5. Browser-only preview | Native renderer, controls/client, feature/dependencies, examples and CI branches removed; unchanged WLED tagged representation |
| 6. Pipeline workers | Owned current-thread processing runtime, tracked WLED task, finite lifecycle with retained resources/deadline failures, final-result drain and native/FFmpeg/thread joins |
| 7. Controller/API | Bounded typed application commands/oneshot replies, retained accepted operations, serialized atomic persistence, conflicts/revisions/rollback, stale-observation rejection |
| 8. Verification/docs | Local checks and browser/protocol evidence below; platform limitations explicitly retained |

No production outputs array, preview-only/null output, session registry, generic graph,
plugin framework, state-management library or restart-setting forms were added.
The unrelated research Python cache remains untouched.

## Final local verification

- `cargo test --locked --workspace`: passed, including native-adapter fixture tests,
  controller HTTP/persistence/rollback and doctests. Hardware-wire fixtures remain ignored.
- `cargo test --locked --workspace --all-features`: 283 passed across 33 test suites,
  zero failures, six opt-in/fixture-dependent tests ignored in that combined run.
  Subsequent targeted checks passed the added persistence-failure regression, stale
  observation rejection and the expanded six-setting/same-connection regression.
- `cargo test --locked -p prysm --lib --test controller -- --ignored --test-threads=1`:
  passed real FFmpeg 9.0.2 EOF, corrupt decoder input, cancellation/reaping, live setting,
  input restart, shutdown and WLED scheduling measurement tests. Physical TV wire data
  is not supplied by this command.
- Strict workspace all-target Clippy with all-features and no-default-features passed.
- Formatting, `git diff --check`, workspace all-feature Rustdoc and 409 documentation
  links passed. The existing `block 0.1.6` dependency emits a future-incompatibility
  notice; current checks pass.
- `npm test --prefix crates/web/prysm-web/control-ui`: generated OpenAPI/type freshness,
  TypeScript, production build, DOM behavior and actual Vite proxy all passed.
- Browser checked the actual executable with synthetic FFmpeg video and a
  loopback WLED fixture at desktop 1440px and narrow 390px. Narrow scroll width stayed
  390px; keyboard Tab moved from brightness to shadow lift. Real Save & apply advanced
  the applied revision without resetting processed frame count. Stop and natural EOF
  retained final imagery with honest stale labels. A real competing REST write preserved
  draft values, displayed saved/draft differences, blocked overwrite and required reload.
  [Desktop preview evidence](../images/preview-tuning.jpg) and
  [narrow Overview evidence](../images/control-panel.jpg) are captured from that fixture.
- Actual WebSocket protocol check: four viewers accepted, fifth rejected, bounded
  frame/busy responses, valid JPEG plus matching metadata, and continuing HTTP/processing
  progress while viewers were idle. Preview tests additionally gate a stalled encoder,
  prove one active/one queued job, latest-snapshot selection, no-demand laziness/cache,
  portrait/BGR geometry and connection shutdown completion. DOM tests gate decode,
  hidden/pause/close teardown and asymmetric bottom/left screen coordinates.
- Native resource-release and stop-deadline tests use explicit gates/acknowledgements.
  Timeout reports failure and retains active resources; restart/final WLED actions are
  prevented until real completion. A capture connection survives all six individual
  processing edits; each revision is acknowledged without capture/output recreation.

## Timing and delivery observations

Same release processor benchmark after integration, with build work finished:
640×360 changing 1.201ms/frame, static 1.176ms, 1920×1080 changing 9.540ms,
full-decode reference 0.367ms. Baseline was 1.266/1.157/9.568/0.373ms respectively.
These are single local runs, not a statistical speedup claim; processor algorithms
were deliberately unchanged. Concurrent-build trials were noisier and excluded from
this comparison.

WLED full resample/packing/nonblocking loopback send, 100 calls: 300 LEDs p50 10.9µs,
p99 66.5µs, max 94.5µs; 65535 LEDs p50 1.75ms, p99 1.92ms, max 1.95ms. This fits
the documented 5ms handler budget on this Mac, so output remains an application
runtime task. No strict scheduling isolation from HTTP is claimed.

`cargo test -p prysm --lib measure_processing_delivery_and_local_age -- --ignored --nocapture`
uses a dedicated processing thread, 240 synthetic 320×180 frames, 300 loopback LEDs,
and an intentional initial 32-frame consumer gate. One observed run processed 240,
delivered 210 and coalesced 30 latest-watch values; local source-stream-poll to output
render-return age p50 1.152ms, p99 1.392ms, max 1.442ms. Delivery counts vary with
scheduler timing; no unbounded queue grows. The test owns its timestamps and adds no
production metadata. Neither this nor JPEG/browser timing measures capture-device,
physical screen-to-light or network acknowledgement latency.

## Platform and hardware limits

No physical camera/TV/WLED calibration or screen-to-light latency was measured.
macOS native-adapter tests use fixtures and cannot establish Linux V4L2 behavior.
An attempted `cargo check --locked -p prysm --target aarch64-unknown-linux-gnu` found
the Rust target installed but failed in native dependencies: missing
`aarch64-linux-gnu-gcc/g++` and `linux/videodev2.h`. Supported Linux CI remains configured.
A subsequent real Linux/aarch64 verification succeeded in a disposable local Docker
container using the existing official rust:bookworm image (Rust 1.99.1) and checksum-verified
official Node 24.22.2. The repository was mounted read-only; build-script writes and
clang/libclang/libudev installation stayed inside a disposable copy/layer.

`cargo test --locked -p prysm -p prysm-capture -p prysm-core -p prysm-processor -p prysm-renderer-wled --lib`
passed 112 tests, zero failures, three ignored (two opt-in timing tests and the FFmpeg 9
fixture). This includes Linux V4L2 missing-device original I/O cause, capture-release
barrier and color metadata plus the new pipeline/lifecycle/controller/preview tests.
Latest application source was synchronized and compared before compilation.
The disposable container was removed; existing containers/images/configuration were
not changed. Commands and logs are retained locally in
`/private/tmp/prysm-linux-20261102/README.txt` and `linux-verification.log`.

This closes the native Linux compile/fixture-test gap. It does not establish physical
camera/TV/WLED behavior, Linux deployment performance, Linux FFmpeg 9 playback,
ARM Debian package installation, or a full Linux/all-feature workspace CI run.
Those remain target/hardware/CI checks; macOS or virtualized Linux timings are not
Raspberry Pi performance guarantees.
