# TV UI capture without slowing video — 2026-11-12

**Status:** measured with a standalone probe on the dev TV; not implemented in v1.
Issue `prysm-3jx.61`. Current behaviour and permissions:
[TV lifecycle and permissions](../../../docs/developers/webos.md).

## Finding

**Yes, if UI capture runs only while there is no video.** The graphics-plane
capture (`libhalgal`) that Prysm removed in 0.1.23 works on this TV as root,
without a Luna role. At 320×180 it costs **4.3 ms per frame**, of which 0.6 ms
is CPU. Including the copy out of the mapped surface and conversion to NV12,
a frame takes about **6 ms and 2.2 ms of CPU**.

While video plays, the existing blended VTCapture tap (mode 3) already
contains the menus drawn over the video. HAL is only needed when `MAIN` has
no video (Home, app browsing without video, settings over no input). So the
two taps never need to run at the same time. Video frames therefore cannot be
slowed: the HAL tap does not exist while video is available.

The old path was slow because it did something else. It captured HAL at the
full video size (640×360), on every frame, in the same loop as video capture
and the H.264 encoder. It also blended UI and video on the CPU.

## Why the old UI path was slow

The removed path is in commit `0e0b7f6^`, in
`crates/capture/webos-capture/src/native/ui.rs` and `src/capture.rs`. It
tried `libgm` (`GM_CaptureGraphicScreen`) first, which this TV does not
have; it ships only `libgm-wayland`. It then used `libhalgal`:
`HAL_GAL_Init`, then `HAL_GAL_CreateSurface(w, h)`, then
`HAL_GAL_CaptureFrameBuffer` into a surface mapped from `/dev/gfx`, then a
copy of that BGRA (premultiplied) surface into a buffer. Every iteration ran
these steps one after another on one thread:

1. Acquire video (0.4 ms).
2. Capture HAL at the video's size (640×360) and copy it.
3. If any UI pixel had alpha above zero, convert video to RGB, blend the UI,
   convert back to NV12 (about 3.2 + 3.0 + 3.6 ms).
4. Hardware H.264 (about 12 ms of output wait).

Measured costs ([live performance](../2026-11-02-encoder-latency/live-performance.md),
[UI split](../2026-11-02-encoder-latency/ui-combined-results.md)):

- UI acquisition took 15.1–15.2 ms per frame: 12.37 ms in
  `HAL_GAL_CaptureFrameBuffer` and 2.80 ms copying.
- This cost was paid on every frame, even when the UI was fully
  transparent.
- With transparent UI the iteration averaged 28.9 ms, against a 33.3 ms
  budget. With a visible overlay it averaged 33.3 ms, 93 of 220 deadlines
  were skipped, and frames were received every 35.7 ms (below 30 fps). The
  sender used 38.8% of one core.

Static analysis traced `HAL_GAL_CaptureFrameBuffer` to KADP Blit or
StretchBlit on the graphics engine. The new numbers confirm it: wall time
grows with the output size, and the CPU is mostly idle during the call. The
copy is also slow because the surface is uncached device memory.

The blended tap made all of this unnecessary while video plays. But it does
not work without video: mode 3 returned `planeInfo failed: 13` on Home
([single-tap cleanup](../2026-11-02-encoder-latency/single-tap-cleanup.md)),
and mode 4 never became ready ([OSD Home](../2026-11-02-encoder-latency/osd-home/README.md)).
HAL was removed along with the CPU blending, so since 0.1.23 Home is not
captured.

## What hyperion-webos does

Inspected revision `00c932f0` (MIT). `unicapture/unicapture.c` runs one
capture thread plus a vsync thread that signals once per frame. On each tick
it acquires the UI (`libgm`, else `libhalgal`) and then the video
(`libvtcapture`, else `libdile_vt`), one after the other. It blends them on
the CPU with libyuv (`ARGBBlend`, or `BlendPlane` for NV12). With no video
frame it sends the UI frame alone. The HAL backend works the same way as
Prysm's old one, except that it reads straight from the `/dev/gfx` mapping
instead of copying first.

It has no separate UI thread, no lower UI rate, no check for unchanged or
transparent UI, and no switching between taps. By default it captures video
from `DISPLAY_OUTPUT`; the blended tap is only a quirk flag. Two things
differ from Prysm's old path:

- The default capture size is **320×180**. HAL's cost depends on output
  size, so this is the main saving (about 4.3 ms instead of 12.3 ms).
- There is no encoder in its loop. It sends raw RGB to Hyperion, so its
  budget does not also hold a 12 ms hardware H.264 wait.

Its HAL backend notes the same thread affinity Prysm found:
`HAL_GAL_Init` binds an EGL context to the calling thread, so init, capture
and destroy must all run on one thread.

## Measurements

Two bounded sessions on November 12, 2026 (16:32 and 16:36 CET), each under
two minutes with Prysm disabled. The TV was an LG RTD2875Q running webOS TV
10.3.2, kernel 5.4 aarch64 with a 32-bit userland, on 4 × Cortex-A73
(`0xd09`). It showed HDMI1 at 3840×2160 / 50 Hz with `MAIN` connected; the
foreground app was `com.webos.app.hdmi1` and `videooutput` reported
`muted: true`. The graphics plane was transparent apart from the mute icon:
211 of 57,600 pixels were visible, and a dumped frame showed the icon. That
image stays local because it is screen content.

`HAL_GAL_CaptureFrameBuffer` copies the whole plane whatever it contains, so
the cost should be the same on Home. Home itself could not be measured; see
Limits.

`probe.c` loads `libhalgal.so.1` and keeps it loaded. It runs init, create,
capture and destroy on one thread, takes `/tmp/prysm-capture.lock`, and
cleans up on SIGTERM. Means below skip five warm-up frames. Raw per-frame
CSVs are in `evidence/run1` and `evidence/run2`.

### Cost per UI frame (back to back)

| Output | HAL call, wall (mean / p95) | HAL call, CPU | Copy from `/dev/gfx` | Visibility scan + hash | BGRA → NV12 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 160×90, 192×108 | Fails (`-1` on the first capture) | — | — | — | — |
| 256×144 | 3.86 / 5.74 ms | 0.67 ms | 0.43 ms | 0.14 ms | 0.38 ms |
| **320×180** (two runs) | **4.33–4.41 / 4.43–6.63 ms** | **0.62–0.76 ms** | 0.72–0.73 ms | 0.22 ms | 0.60 ms |
| **640×360** (two runs) | **12.32–12.33 / 12.33–12.39 ms** | 1.49–1.67 ms | 2.93–2.96 ms | 0.91–0.92 ms | 2.73 ms |
| 1280×720 | 29.11 / 29.67 ms | 3.34 ms | 11.82 ms | 3.51 ms | not run |

The 640×360 HAL time matches the 12.37 ms measured in 0.1.21, so the
platform has not changed. The call is waiting on the graphics engine: CPU
time is about 15% of wall time. At 30 Hz the call takes a steady 4.27 ms
(320×180) and 12.27 ms (640×360), which does not look like waiting for
vsync. The smallest size that works is between 192×108 and 256×144.

### Paced capture, one thread

| Workload | CPU, share of one core |
| --- | ---: |
| 320×180 at 10 Hz (capture, copy, scan) | 1.8% |
| 320×180 at 30 Hz (capture, copy, scan) | 5.8% |
| 640×360 at 30 Hz (capture, copy, scan, NV12) | 24.3% |

From the per-step means, 320×180 at 30 Hz with NV12 conversion would use
about 7.5% of one core. That was not run as a single measurement.

### Opening and closing (10 cycles, 320×180)

| Step | Median | Max |
| --- | ---: | ---: |
| Init + create + map | 0.61 ms | 4.09 ms |
| First capture | 4.33 ms | 5.60 ms |
| Destroy + unmap | 0.15 ms | 0.17 ms |

Calling `HAL_GAL_Init` again in the same process worked in every cycle.
Earlier lifecycle work found that the library must stay loaded: unloading it
leaks file descriptors ([cleanup diagnosis](../2026-11-01-lifecycle/cleanup-diagnosis.md)).
Switching between the video and UI taps therefore adds about 5 ms on the UI
side, on top of the existing VTCapture release and open.

### Background UI thread next to a simulated 30 fps video loop

The real video path could not run next to the probe. VTCapture needs a
Luna role tied to the executable's path, and registering one was not
allowed here.

Instead, the main thread stood in for the video loop. It woke on a fixed
33.333 ms clock and copied and read a 640×360 NV12 buffer. A second thread
ran HAL capture, copy and scan. These are 10-second windows of 300 frames
each.

| UI thread | Video loop interval p99 / max | Late wake-up p99 / max | Process CPU |
| --- | ---: | ---: | ---: |
| None (two runs) | 33.37 / 33.38–34.17 ms | 0.09–0.42 / 0.11–0.91 ms | 0.6% |
| 320×180 at 10 Hz | 33.40 / 34.42 ms | 0.15 / 1.16 ms | 2.2% |
| 320×180 at 30 Hz | 33.37 / 33.67 ms | 0.09 / 0.41 ms | 5.0% |
| 640×360 at 30 Hz (two runs) | 35.89–36.28 / 36.00–36.30 ms | 2.63–3.02 / 2.73–3.04 ms | 13.4–14.0% |

A concurrent 320×180 UI thread had no visible effect on the stand-in loop.
At 640×360 it made the loop wake up to about 3 ms late at p99. The most
likely cause is memory or driver contention from the larger uncached copy,
since four cores were not saturated. This argues against running HAL
alongside video even on its own thread. This is a CPU-side proxy: it does
not show whether HAL slows the VTCapture tap or rvenc.

## Recommendation

Bring UI capture back as a second **tap kind** that the adapter's existing
native thread opens only when video is unavailable. This needs no new
thread or composition step, and video is unaffected by construction.

- **Selection.** `include_ui = true` changes meaning from "video + menus" to
  "video + menus, or the TV UI when there is no video":
  - `Video::Available` or `Unknown`: blended VTCapture tap, as today.
  - `Video::Unavailable`: HAL graphics tap. Today this case releases the tap
    and sleeps.
  - `include_ui = false`: unchanged.

  A change in availability or generation already releases the current tap.
  The next `step()` would open the kind that matches the new state.
- **Threading.** `Native::run` is already a private thread that owns the
  LG tap, its watchdog (5 s per native call), pacing, retries and certified
  release. HAL's EGL thread affinity needs exactly that. Taps never overlap,
  so the device lock and stop/drain/join order stay as they are. No helper
  thread is needed, which follows "adapters are plain objects".
- **Format and size.** Capture HAL at 320×180, or at the configured size if
  that is smaller (256×144 works; 192×108 does not). In one pass, read the
  uncached surface, treat premultiplied BGRA over black as plain RGB, and
  write limited-range BT.709 NV12 (`TAP_FORMAT`). The sampler then gets the
  same format, and the slim H.264 sender keeps NV12; its encoder already
  restarts when the frame size changes. Expected cost is about 6 ms per frame
  and about 7.5% of one core at 30 fps. Do not capture HAL at 640×360: it
  costs 18–19 ms per frame and about 24% of one core, for no benefit to edge
  sampling.
- **FFI.** HAL goes in `capture-webos-sys` next to VTCapture. It loads
  `libhalgal.so.1` once and keeps it for the life of the process. Destroying
  the surface and unmapping it is the certified release. Errors:
  - Failed creation or a nonzero capture status are recoverable faults.
  - A missing library or missing `/dev/gfx` means UI capture is
    unsupported: report it once and keep today's "no frames without video"
    behaviour.
- **Freshness.** All-transparent UI gives a black frame, which matches the
  screen. Use the visibility scan only for metrics; it must not skip
  capture, because detecting a change requires capturing first.
- **Not recommended:**
  - Running HAL alongside video at a lower rate on a helper thread. It is
    unnecessary because the blended tap already holds the menus, and the
    640×360 proxy showed jitter.
  - CPU blending of HAL over the display tap. This was the old cost.

To qualify on hardware before shipping:

- Home or an app with `MAIN` disconnected, power on: visible frames,
  colours, and the HAL cost at that point.
- Home → HDMI → Home switch times, and no device or FD growth over repeated
  switches.
- Screen saver and standby: the host should keep the source closed while
  power is not active.
- A soak test with the real v1 source, checking that video frame intervals
  and latency are unchanged.

## Platform and limits

- One TV, one firmware, two short sessions. HDMI video with a nearly empty
  graphics plane was on screen for every measurement. **Home without video
  was not measured**, because the screen could not be changed in this task.
  Earlier evidence that HAL produced about 30 UI frames per second on Home
  with `MAIN` disconnected comes from version 0.1.8
  ([deployment history](../deployment-history.md)).
- The actual VTCapture path and rvenc were not run next to HAL (no
  temporary Luna role was allowed). The "no effect on video" claim rests on
  the design never running both taps at once. The thread proxy only
  supports it.
- HAL ran as root with no Luna role. Earlier lifecycle runs labelled
  `source-ui` also ran without a role. An installed v1 still runs as root
  and needs `/dev/gfx`.
- The colour accuracy of HAL output compared with the VT tap was not
  checked. HDR, DRM-protected app UI, and the cost when the plane is fully
  opaque are also unknown.

## Licence

The HAL surface layout (`HAL_GAL_SURFACE`) and the call sequence match
hyperion-webos (MIT, `unicapture/backends/libhalgal.c`, `halgal.h`) and
Prysm's removed 0.1.22 backend. `probe.c` is new code that declares the same
ABI; no hyperion code was copied. A v1 implementation that takes the layout
from hyperion's header must keep `LICENSE.hyperion` in `capture-webos-sys`,
which is already there. The vendor libraries are loaded at runtime and are
not redistributed.

## Reproduction and evidence

- [`probe.c`](probe.c) and [`build.sh`](build.sh) (webOS SDK, softfp NEON).
- [`run-tv.sh`](run-tv.sh) runs a matrix file in one bounded session. It
  copies the probe to `/media/developer/temp/ui-probe/`, disables Prysm
  with `setEnabled`, runs every probe under `timeout`, then re-enables
  Prysm and removes the folder in an `EXIT` trap.
- [`matrix-1.txt`](matrix-1.txt) and [`matrix-2.txt`](matrix-2.txt) list the
  two sessions.
- `evidence/run1/`, `evidence/run2/`: per-frame CSVs and summaries, stderr,
  and `videooutput` status before and after each run.
- [`evidence/SHA256SUMS`](evidence/SHA256SUMS): binary hashes. Session 1
  ran the probe before the NV12 conversion step was added.

After both sessions, Prysm reported `session: running`, `input.fps: 30`,
`target: processed`, and the probe folder was gone.
