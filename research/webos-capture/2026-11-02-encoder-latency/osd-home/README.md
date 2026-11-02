# OSD-only cold start on Home — 2026-11-02

**VTCapture mode 4 did not capture Home with no active video on this TV.**
It produced no frame at all; this was not a black or stale-frame result.

The installed 0.1.23 sender was stopped. Home was launched, with foreground Home
and MAIN disconnected (width, height and frame rate all zero) verified before
and after each probe. Each probe was a fresh process holding the capture lock,
requesting mode 4, 640×360, 30 FPS and three buffers. No HAL/GM capture was opened.
“Cold start” here means a newly created capture driver, not a TV power cycle.

Two initial one-shot trials returned init=0 and preprocess=1. Because production
handles status 1 as pending, a second pair of fresh processes retried preprocess
once per second: all 11 calls over ten seconds returned 1 in each process. Both
exited normally through finalize; neither reached plane/buffer acquisition.
No watchdog timeout occurred. No frame files were produced on Home.

As a positive control, the original HDMI1 input was restored. The **same binary
and mode 4** initialized on its first attempt, reporting 640×360 with stride 640,
and captured 90 frames in 3.030 seconds. Inspection of saved sample 45 showed the
HDMI video picture, not an isolated Home/OSD image. A control toast request was
accepted but absent from that sample; no simultaneous HAL positive control was
used in this test. The earlier tap comparison independently documented that
mode 4 omitted a toast visible in HAL while mode 3 included it.

This rules out mode 4 as a working cold-start Home fallback through the tested
libvtcapture API/configuration. It does not establish that the hardware lacks
an OSD path: the mode may be unsupported, ignored, or routed differently by this
firmware. No new backend or runtime capture behavior was added.

## Restoration

The original HDMI1 app was restored. Temporary Luna roles and TV staging files
were removed, the installed sender restarted, and saved configuration bytes
verified unchanged. Final state: enabled, connected, capturing, error null,
Combined hardware H.264 to `192.168.18.18:19400`, metrics/autostart retained.
A subsequent two-second check saw 61 more Blended frames and 60 more frames
processed on the computer.

## Reproduction and evidence

`probe.cpp` reuses the existing native ABI, geometry validation and capture lock;
`build.sh` targets the existing relocated SDK. Copy these files into
`work/webos-osd-home/retry/`, build from the repository root, then run
`python3 work/webos-osd-home/retry/run-local.py` against the authorized TV.
The runner serializes tests, uses bounded process timeouts, and restores input
and sender in `finally`. It expects the prior evidence output files not to exist.

The archived `build.sh` and its provenance hash record the pre-refactor capture
header path. On the current layout, change its include directory to
`apps/prysm-webos-capture/native` before rebuilding.

`evidence/` retains logs, readiness/foreground snapshots, control frame timings,
and restored status. `provenance.json` contains source/binary hashes;
`recovery.json` contains post-restoration frame deltas. Raw TV images remain
local under `work/webos-osd-home/` and are not included in the research tree.

Not tested: other TV models/firmware, other resolutions, undocumented driver
parameters, or alternative native OSD APIs.
