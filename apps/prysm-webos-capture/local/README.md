# Prysm on webOS

This experimental executable opens LG’s blended capture tap, converts its limited-range BT.709 NV12 pixels to RGB, runs the Prysm processor with a local settings file, and sends DDP directly to WLED. It shares the installed worker’s exclusive capture lock. The standalone research build has no video transport or encoder.

[Research checkpoint, tested binary and replay fixtures](../../../research/webos-capture/2026-11-04-on-tv-pipeline/README.md)

With the `embedded-prysm` feature, the TV supervisor runs the shared Prysm portal in this executable. The portal stays up while capture is paused. Native frames enter the shared pipeline directly. [Embedded setup](../../../docs/users/webos.md#processing-on-this-tv-experimental)

## Initial TV trial

A looping bright/dim video ran on the TV on November 4, 2026. Each capture size ran for about 25 seconds at a requested 30 FPS using the same color profile, processing settings and 162-LED layout. The FPS column uses the change in successful DDP frame sends between the first and last five-second progress reports. The controller column is the observed range of the first LED’s green channel from WLED’s independent live preview.

| Native capture | Frames sent | Steady FPS | WLED green range |
| --- | ---: | ---: | ---: |
| 1280×720 | 259 | 10.65 | 51–196 |
| 640×360 | 625 | 25.41 | 48–194 |
| 320×180 | 743 | 29.92 | 51–191 |

WLED identified the TV’s IP as its realtime DDP source at each size. The installed TV service was paused for the trial. Its configuration, the receiver settings and the previous foreground app were verified after cleanup. The temporary executable and Luna role were removed. The trial measured throughput and controller pixel changes. Physical latency, long-run stability and suspend/resume are unmeasured.

The first build used the packaging script’s standard Rust CPU flags. At 1280×720 it sent five frames in 25.7 seconds. The build below enables hardware floating-point instructions while keeping the webOS soft-float calling convention. Rust currently warns that these target-feature switches are unstable. The SHA-256 of the executable used for the table is `1b3bf930f4b07880d4726f835240974ce753115b87acab9290487d5dc3bd0b1f`.

## 640×360 profile

The serial build reproduced 25.34 FPS at the configured density of 256 samples per 1000 pixels. Temporary monotonic timers measured each successful frame from acquisition through DDP send. Both profiled runs used the same looping bright/dim clip and excluded the first five seconds. The serial run contributed 635 frames and the parallel run 900. Values are elapsed milliseconds per frame.

| Stage | Serial mean | Parallel mean |
| --- | ---: | ---: |
| Native acquisition and copy | 0.37 | 0.37 |
| NV12 to RGB | 2.81 | 2.82 |
| Grid averaging | 6.65 | 6.73 |
| Projecting grid colors onto the edges | 28.80 | 11.06 |
| Remaining processing | 0.69 | 0.73 |
| WLED DDP send | 0.14 | 0.14 |
| **Total work, mean / p95** | **39.46 / 42.62** | **21.86 / 25.78** |

At 640×360 the spatial sampler computes 510 edge samples, each a weighted sum over 9,216 cells. The local binary enables the processor’s optional Rayon pool to calculate independent samples concurrently. Each sample keeps the serial accumulation order. In the parallel run, all 900 measured frames finished their work inside the 33.33 ms budget. Scheduling the capture loop against successive deadlines also avoids accumulating normal wake-up delays.

The final binary ran a 90-second moving multicolor pattern with the same calibration, smoothing, density and 162-LED layout. It sent 2,685 frames in 90.215 seconds including startup, and sustained 29.93 FPS between the five- and 90-second reports. It averaged 1.32 CPU cores over the sampled interval. WLED reported the TV as its DDP source and its live pixel readback changed. An independent ten-second observation on the TV saw 301 outbound DDP packets at 29.94 FPS. Every consecutive LED payload changed. The observed packet gap was 37.35 ms at p95 and 57.22 ms at maximum.

The final one-minute repeat of the original bright/dim clip measured 29.997 FPS over 50.172 steady-state seconds, up from 25.34 FPS on the same clip. WLED again identified the TV as its DDP source. The installed service, configuration and foreground app were restored after testing. The SHA-256 of the final tested binary is `7d39aaa263376b5791acc40602b0b2b3a1166c6421a3dfcc14fd175ff19dd908`.

## 1280×720 profile

The same optimized binary (commit `23dac94`) sustained **16.95 FPS** at 1280×720, compared with 10.65 FPS in the initial serial trial. The native driver confirmed the capture dimensions. The test used the same 640×360 bright/dim video, calibration, density and 162-LED layout as the 640×360 trial. The uninstrumented binary sent 1,009 frames in 60.15 seconds including startup. Between the first and last progress reports it sent 854 frames in 50.392 seconds, using an average of 1.72 CPU cores over the CPU sampling interval. WLED identified the TV as its DDP source. A ten-second packet observation measured 16.89 FPS independently.

A separate 45-second run with temporary monotonic timers sustained 16.74 FPS. The following times cover 675 successful frames after the first five seconds.

| Stage | Mean ms | p95 ms |
| --- | ---: | ---: |
| Native acquisition and copy | 0.71 | 0.81 |
| NV12 to RGB | 11.10 | 13.67 |
| Grid averaging | 23.15 | 26.35 |
| Projecting grid colors onto the edges | 21.97 | 30.29 |
| Remaining processing | 2.38 | 2.92 |
| WLED DDP send | 0.14 | 0.18 |
| **Total work** | **59.45** | **70.49** |

At the configured density and full viewport, 720p has four times the input pixels of 640×360 and 1,022 edge samples instead of 510. Both project from 9,216 grid cells. Grid averaging still runs serially, while edge projection uses the worker pool. Together these two stages consumed 45.12 ms per frame. Even the fastest measured frame took 52.38 ms against the 33.33 ms budget for 30 FPS. The settings, installed service and original Home foreground were verified after both runs.

## Parallel grid and ARM conversion at 1280×720

On the original brightness clip, the on-TV pipeline reached **29.97 FPS** over 35.170 steady-state seconds, up from 16.95 FPS on the previous 720p trial. The settings snapshot, 162 LEDs, native tap, and video clip were the same. Enabling parallel grid averaging and the YUV library’s parallel row conversion first reached 26.45 FPS. The webOS build now uses an ARMv7 NEON kernel for eight-pixel blocks with the same limited-range BT.709 Q13 coefficients and rounding. Other widths use the portable converter. Each grid cell traverses contiguous RGB rows in its original summation order. Two projected edge samples share each cell load and accumulate separately. The capture clock keeps its phase across short overruns.

The stage profiles below omit the first five seconds. The earlier profile included 675 frames and the final one included 812. Times are milliseconds per successful frame.

| Stage | Previous mean | Final mean | Final p95 |
| --- | ---: | ---: | ---: |
| Native acquisition and copy | 0.71 | 0.69 | 0.80 |
| NV12 to RGB | 11.10 | 2.94 | 3.92 |
| Grid averaging | 23.15 | 3.34 | 5.17 |
| Edge projection | 21.97 | 16.06 | 19.87 |
| Other processing | 2.38 | 1.41 | 2.61 |
| DDP send | 0.14 | 0.14 | 0.18 |
| **Total work** | **59.45** | **24.59** | **29.07** |

The TV’s ARM test compares the vector kernel with the portable converter for every 8-bit Y, U and V combination. A separate replay ran both the original and optimized pipelines on 18 deterministic NV12 frames at fixed processing timestamps and with the active calibration. It included black, white, random chroma, dark borders and two frame sizes (1280×720 and 322×182). All 34,232,472 RGB bytes and all 8,928 bytes of WLED DDP packets matched using a bytewise comparison.

The final executable sustained **29.86 FPS** between the first and last five-second reports of a 90-second moving multicolor clip. It sent 2,671 frames including startup and averaged 2.02 CPU cores. WLED identified the TV as its DDP source. A separate 15-second packet capture saw 443 packets at 29.45 FPS, with a change in LED values across all 442 consecutive pairs. Packet gaps reached 43.69 ms at p95 and 56.63 ms at maximum. The original foreground app, receiver settings and installed service were restored. The moving-clip executable’s SHA-256 is `c9ea0446c6514174d02d0c5959d1844cb2f6b06aa8dbd8347530ca2ea81af3c3`.

## Build and run

From the worktree root with the Rust ARMv7 target installed and `WEBOS_SDK` pointing to the relocated buildroot-nc4 SDK, run:

```sh
apps/prysm-webos-capture/local/build-tv.sh
```

The output is `target/armv7-unknown-linux-gnueabi/webos/prysm-webos-local`. As root on the TV, run `prysm-webos-local SETTINGS.json` under a Luna capture role scoped to the executable’s actual path. Use the inner `settings` object from Prysm’s settings response. It supplies capture rate and resolution, output address, LED layout, processing and calibration. Stop the installed capturer before starting the trial, provide an active video source, and send SIGTERM to release the tap. The binary runs in the foreground. Autostart, power management and a settings UI are outside this experiment.
