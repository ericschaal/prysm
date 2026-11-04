# Prysm on webOS

This experimental executable opens LG’s blended capture tap, converts its limited-range BT.709 NV12 pixels to RGB, runs the Prysm processor with a local settings file, and sends DDP directly to WLED. It shares the installed worker’s exclusive capture lock. The normal dependency tree contains no OpenH264, GStreamer or WebSocket implementation.

## TV trial

A looping bright/dim video ran on the TV on November 4, 2026. Each capture size ran for about 25 seconds at a requested 30 FPS using the same color profile, processing settings and 162-LED layout. The FPS column uses the change in successful DDP frame sends between the first and last five-second progress reports. The controller column is the observed range of the first LED’s green channel from WLED’s independent live preview.

| Native capture | Frames sent | Steady FPS | WLED green range |
| --- | ---: | ---: | ---: |
| 1280×720 | 259 | 10.65 | 51–196 |
| 640×360 | 625 | 25.41 | 48–194 |
| 320×180 | 743 | 29.92 | 51–191 |

WLED identified the TV’s IP as its realtime DDP source at each size. The installed TV service was paused for the trial. Its configuration, the receiver settings and the previous foreground app were verified after cleanup. The temporary executable and Luna role were removed. The trial measured throughput and controller pixel changes. Physical latency, long-run stability and suspend/resume are unmeasured.

The first build used the packaging script’s standard Rust CPU flags. At 1280×720 it sent five frames in 25.7 seconds. The build below enables hardware floating-point instructions while keeping the webOS soft-float calling convention. Rust currently warns that these target-feature switches are unstable. The SHA-256 of the executable used for the table is `1b3bf930f4b07880d4726f835240974ce753115b87acab9290487d5dc3bd0b1f`.

## Build and run

From the worktree root with the Rust ARMv7 target installed and `WEBOS_SDK` pointing to the relocated buildroot-nc4 SDK, run:

```sh
apps/prysm-webos-capture/local/build-tv.sh
```

The output is `target/armv7-unknown-linux-gnueabi/webos/prysm-webos-local`. As root on the TV, run `prysm-webos-local SETTINGS.json` under a Luna capture role scoped to the executable’s actual path. Use the inner `settings` object from Prysm’s settings response. It supplies capture rate and resolution, output address, LED layout, processing and calibration. Stop the installed capturer before starting the trial, provide an active video source, and send SIGTERM to release the tap. The binary runs in the foreground. Autostart, power management and a settings UI are outside this experiment.
