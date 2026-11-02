# Live 640×360 performance and UI capture alternatives

Read-only observation on 2026-11-02, 10:26:55–10:29:55 Europe/Paris,
after deployment of 0.1.21. Requested configuration: Both, 640×360, 30 fps,
hardware H.264, 2 Mbps. Sender and capture settings were not changed.

## Measured budget

Both Prometheus targets stayed up, with no observed duration-counter resets.
The query window contains 36 scrapes spanning 175 seconds.

| Stage | Mean ms |
| --- | ---: |
| Whole TV iteration | 28.928 |
| UI acquire, including mapped-buffer copy | 15.114 |
| Encoder prepare payload | 12.333 |
| ↳ Hardware wait, included above | 11.937 |
| Video acquire | 0.408 |
| Socket write | 0.140 |
| Host H.264 decode | 1.123 |
| Host processing | 2.491 |

The 33.333 ms frame budget leaves **4.406 ms / 13.2% average headroom**.
Host receive and decode rates were 29.989 fps. Raw same-scrape counters show
23 skipped deadlines in 5,249 iterations (0.438%); these are scheduling
deadlines, not a packet-loss count. Twenty iterations exceeded the 33.4 ms
histogram boundary; none exceeded 50 ms. Histogram buckets are too coarse
to claim precise p95/p99 headroom.

The TV sender used 0.314 CPU cores on average. Total TV CPU utilization was
15.95%. These measurements do not suggest whole-CPU saturation. Host header
reads spend time waiting for the next frame and are not processing overhead.

All encoded-frame increments in this window were tagged Video, while UI
acquire continued at 29.989 fps. The composition code passes native video
through when all UI alpha values are zero. Thus this workload pays for UI
capture every frame even though the resulting UI is transparent. Visible-menu
composition may cost more and was not measured in this window.

## Why UI acquisition is expensive

`native/ui.rs` wraps both `HAL_GAL_CaptureFrameBuffer` and the subsequent
mapped BGRA copy in one UiCapture/Acquire measurement. The mapping and output
allocation are reused. A 640×360 BGRA frame is 921,600 bytes; copying it at
30 fps is 27.65 MB/s, but the actual mapping's copy performance is unmeasured.

Static inspection of the retained TV libraries establishes this path:

- `HAL_GAL_CaptureFrameBuffer` at 0x3320 delegates to KADP.
- `KADP_GAL_CaptureFrameBuffer` at 0x67c78 queries the framebuffer with an
  ioctl, then calls Blit at 0x67d88 or StretchBlit at 0x67db4 depending on geometry.
- `KADP_GAL_StretchBlit` at 0x66d7c contains graphics-engine ioctls and a
  `RHAL_SE_ExecCmd` call at 0x674e2.

This is an on-demand graphics operation followed by a CPU copy. Video capture
reads from an already-produced native NV12 buffer. That structural difference
is a plausible explanation for their different costs, but we have **not split
HAL execution from copying**, identified the live blit branch, or established
a vsync wait. Calling the full 15.1 ms either memcpy time or GPU time would be
unsupported. ARM/Thumb PLT labels were checked against the retained corrected
relocation map rather than trusting automatic disassembler names.

## Alternatives and next experiments

| Option | Expected benefit and limitation |
| --- | --- |
| VT `BLENDED_OUTPUT` | Best backend hypothesis: capture video and UI together and eliminate separate HAL acquisition/composition. Upstream exposes this mode, but this TV's support, output format, colors and overlay completeness are untested. |
| VT `OSD_OUTPUT` | Potential alternate UI source; likewise unqualified on this TV. |
| UI and video acquire concurrently | Only about 0.4 ms ideal saving here because video acquire is already cheap. |
| UI acquisition overlapping previous-frame encoding | Could hide part of the 12.3 ms encode interval and improve throughput. Requires bounded buffers and explicit frame freshness; does not automatically shorten current-frame latency. Keep HAL init/capture/destruction on its owning thread. Shared-driver contention is unmeasured. |
| UI at 15 Hz, video at 30 Hz | Nominal average UI cost halves, saving about 7.6 ms per output frame, but UI updates can lag by a refresh interval (~67 ms) plus capture/scheduling time. Reusing a frame requires accepting this tradeoff. |
| Video-only capture | Existing option avoids UI work but omits menus and overlays. No setting was changed. |
| GM or DILE VT | Previous TV evidence found no GM library and nonfunctional DILE VT entry points; neither is an established replacement here. |
| Luna screenshot API | OSE documentation describes file-oriented screenshots, no scaling and possible exclusion of video. Poor candidate for a continuous 30 fps feed; OSE availability does not establish availability on this TV. |

Recommended order: split the HAL-call/copy timings, then run a bounded VT
blended/OSD capture probe with visible overlays and decoded-frame verification.
Only introduce capture/encode pipelining if those simpler experiments fail to
remove the expensive path. No supported UI damage-notification mechanism was
established; detecting unchanged pixels after capture does not save capture.

## Upstream API repository

[tv-native-apis](https://github.com/webosbrew/tv-native-apis) explicitly marks
itself deprecated in favor of
[webos-userland](https://github.com/webosbrew/webos-userland).
Inspected revision: `00a5f8721e15d5c10030b99b2f3e73e0c527bcce`.
Its README and build definitions identify headers and **link stubs**, not
replacement implementations or open hardware drivers. `src/dummy.c` supplies
dummy symbols used to build the stub libraries. Using these at build time
still relies on the actual vendor libraries at runtime.

The inspected tree has DILE VT headers, EGL/GLES and media APIs, but no
HAL GAL, GM or libvtcapture definitions. Its DILE VT dump enum lists scaler
input/output and display output, not the extra blended/OSD modes exposed by
Hyperion's libvtcapture backend. It is useful as the maintained API reference,
but is not a complete drop-in source for our current capture bindings.

Primary references:

- [Pinned userland build definitions](https://github.com/webosbrew/webos-userland/blob/00a5f8721e15d5c10030b99b2f3e73e0c527bcce/CMakeLists.txt)
- [Pinned DILE VT header](https://github.com/webosbrew/webos-userland/blob/00a5f8721e15d5c10030b99b2f3e73e0c527bcce/include/dile/dile_vt.h)
- [Hyperion libvtcapture modes](https://github.com/webosbrew/hyperion-webos/blob/main/unicapture/backends/libvtcapture.cpp)
- [OSE compositor capture API](https://www.webosose.org/docs/reference/ls2-api/com-webos-surfacemanager/)

## Evidence

Working evidence is under `work/webos-encoder-latency/live-performance/`:
`query.py`, `snapshot.json`, `summary.txt`, `raw-loop-counters.json`,
`kadp-gal-capture.asm`, `kadp-gal-stretch.asm`, and pinned `userland/` sources
and repository-tree metadata. Raw Prometheus data includes local network
addresses and remains in the ignored work directory.

HAL ELF SHA-256:
`2ae4d20dc5759aa9333cd09a15b02a9cd39aecf3c573e3fb8341f8ea395d29cb`.
KADP ELF and corrected PLT mapping are retained under
`work/webos-encoder/evidence/collection.ciJyC0/`.
Earlier backend availability evidence is in
`research/webos-capture/2026-11-01-lifecycle/cleanup-diagnosis.md`.
