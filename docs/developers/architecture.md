# Architecture

```text
Capture device → Prysm application → WLED renderer → LEDs
                       ↕
                  browser panel
```

An HDMI card uses the **V4L2 capturer** on Linux or **AVFoundation capturer** on
macOS. The **webOS capturer** can send video to a separate Prysm host. With experimental embedded processing, the TV runs the same portal and lighting pipeline and supplies raw frames directly. Video files use FFmpeg on host installations.

## Ownership and boundaries

| Owner | Responsibility |
| --- | --- |
| [Application](../../apps/prysm/runtime/src/application.rs) | HTTP, controller, preview and receiver lifetime. Starts idle; HTTP survives capture failure. |
| [Settings store](../../apps/prysm/runtime/src/settings.rs) | Validate JSON, check revisions, write atomically before publishing changes. CLI/environment process options remain separate. |
| [Controller](../../apps/prysm/runtime/src/control/controller.rs) | Serialize settings/power/source changes; accepted work survives caller disconnects. |
| [Input boundary](../../apps/prysm/runtime/src/input.rs) | The host opens a device, file or remote receiver. The embedded executable supplies a native frame stream, its availability and a handle for waiting on cleanup. |
| [Pipeline](../../apps/prysm/runtime/src/pipeline/) | One input, processor and WLED output. Processing has its own OS thread/current-thread Tokio runtime; WLED runs alongside HTTP. |

## Processing and color

`PrysmProcessor`: black-bar detection → sampling → smoothing → shadow lift →
calibration → brightness. `ProcessedSnapshot` pairs source pixels, crop and colors.
Lighting receives only `EdgeColors`. Frames favor the latest value; lifecycle
commands stay ordered. Processing edits apply between frames; input/output edits
restart the run.

Frames share immutable pixels. Each producer retains up to four pool buffers,
reusing only released buffers; held readers never block capture.
Core has no HTTP, serialization or runtime dependencies. Contract types validate
user input. Processing assumes sRGB transfer; YUV matrix metadata is not HDR
transfer decoding. File input tone-maps first.

## Preview observation

Preview never starts capture. A demand-driven JPEG worker retains the last paired
snapshot across stop. Protocol and limits: [preview API](control-api.md#browser-preview-and-live-settings).

## Shutdown ordering

Cancel capture → finish processing → drain final output → observe native/FFmpeg
cleanup → join worker → send WLED Off/effects. A five-second timeout is failure:
retain handles and block restart until cleanup completes. Output failure cancels
capture and retains its cause. Preview shuts down separately.

## Workspace map

`apps/` owns application lifetimes. `crates/capture`, `processing` and `rendering`
own the video path; `crates/common` owns data/contracts/metrics; `crates/web` owns
both frontends. TV [worker](../../apps/prysm-webos-capture/README.md) and
[supervisor](../../apps/prysm-webos-service/README.md) run as separate processes. The supervisor selects the remote capture worker or the embedded executable that owns the portal.
