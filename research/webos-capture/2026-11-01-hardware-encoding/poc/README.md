# Prysm webOS hardware H.264 encoding PoC

**Target: encoding on the webOS TV.** This is a synthetic research PoC,
not a production backend.

Contains the working synthetic TV probe, original and SPS-adjusted H.264
fixtures, and a checker using Prysm's unchanged receiver. Keep this directory
at `research/webos-capture/2026-11-01-hardware-encoding/poc/` inside Prysm so the checker's relative
path dependency resolves.

## Offline check

Run from the Prysm repository root:

```bash
POC=research/webos-capture/2026-11-01-hardware-encoding/poc
python3 "$POC/probe.py" --self-check
cargo run --offline --locked --manifest-path "$POC/receiver-check/Cargo.toml" --target-dir /tmp/prysm-webos-hardware-encoding-poc-target -- "$POC/fixtures/access-units"
```

The self-check does not load native libraries or touch a device. Cargo needs
the existing dependencies cached for `--offline`; this installs nothing.
Expected: original SPS rejected for dimensions and BT.709; all fifteen
corrected hardware access units decode with matching grayscale RGB pixels.
Original check results and clip metadata are in `evidence/`.

## Working synthetic contract

| Item | Observed run |
|---|---|
| Input | 640x360 NV12, 15 FPS, fifteen grayscale frames |
| Allocation | `/dev/dma_heap/dvr`; one DMA-BUF for both planes |
| Row pitch | 768 for both planes; must be divisible by 192 |
| Chroma offset / allocation size | 276480 / 414720 bytes |
| Metadata | GstVideoMeta with explicit offsets and strides |
| Encoder | `appsrc ! rvenc target-bitrate=2000000 ! h264parse` |
| Output | 16286 bytes; Constrained Baseline, progressive; 15 frames |
| Teardown | EOS, stop, close, finalize; normal exit 0 before watchdog |
| Receiver | Original rejected; SPS-only correction passes unchanged decoder |

[RUN-ON-TV.md](RUN-ON-TV.md) describes bounded reproduction. Offline commands
above perform no SSH, service action or hardware test.

## Required SPS correction

Original output advertises 640x368 with no bottom crop and no video-signal
color description. The adjusted fixture declares 640x360, limited range and
BT.709. Every non-SPS NAL is byte-identical: pictures were not re-encoded.
To recreate the adjusted fixture with installed FFmpeg and a fresh output:

```bash
ffmpeg -v error -n -i research/webos-capture/2026-11-01-hardware-encoding/poc/fixtures/pitch-768.h264 -map 0:v -c copy -bsf:v h264_metadata=crop_bottom=8:video_full_range_flag=0:colour_primaries=1:transfer_characteristics=1:matrix_coefficients=1 -f h264 /tmp/prysm-hardware-encoding-sps-adjusted.h264
```

In production, derive cropping from actual coded/visible dimensions; eight
rows is specific to this example. Signal BT.709 only for actual limited-range
BT.709 input/conversion. Keep receiver validation intact. FFmpeg here is an
offline demonstration, not a proposed production subprocess dependency.

## Implementation touchpoints

- `crates/capture/webos-capture/src/encoder.rs`: sender encoder variants,
  frame validation, output/header ownership and existing metrics.
- `crates/capture/webos-capture/src/session.rs`: encoder construction once per
  running session and capture/encode/send/reconnect/shutdown ownership.
- `crates/capture/prysm-webos-protocol/src/h264.rs`: current RGB/NV12/NV16
  conversion behavior and unchanged receiver SPS/colorimetry requirements.
- `crates/capture/webos-capture/src/native/mod.rs`: dynamic-library ownership
  and error patterns; libraries outlive native resources and callbacks.
- `crates/capture/prysm-capture/src/webos.rs`: host receiver connection and
  ordered decode tests; not the TV encoding implementation boundary.

Prefer the demonstrated GStreamer boundary for the initial backend. A direct
private-ABI backend still needs output callback/buffer lifetime reconstruction.
Reuse installed libraries and existing dependencies; avoid a generic framework
or new user-facing options without need. Preserve software operation on
unsupported hosts and failures. Do not switch reference chains midstream
without resetting the connection/decoder state.

## Acceptance checks

1. Real RGB/NV12/NV16 capture inputs preserve visible dimensions and colors,
   including non-macroblock-aligned heights. Allocate/copy padding safely;
   uniform probe pixels do not prove arbitrary-plane padding behavior.
2. Each wire payload is one complete Annex B access unit. Match asynchronous
   output to input; reconnect starts with an independently decodable IDR/SPS/PPS.
3. SPS satisfies the unchanged receiver. Test crop calculation, limited BT.709,
   dimensions, malformed and repeated parameter sets.
4. Bounded startup/drain/shutdown; no DMA fd or callback leaks; no early library
   unload. Exercise stop, standby, reconnect and resolution changes.
5. Measure capture+encode+send latency, CPU and dropped frames against OpenH264;
   qualify simultaneous native capture before enabling hardware by default.

Unproven: colored input, real capture conversion, timestamps (vendor callbacks
repeated one PTS), repeated restart, coexistence, throughput or CPU savings.
This firmware has ARM32 EABI5 userland on an AArch64 kernel.

## Full evidence

Detailed call/descriptor maps, firmware hashes and live logs remain in
[the detailed report](../report.md).
Vendor library dumps are absent from this package. Kodi's inspected Starfish/
ACB path is managed compressed playback, not this raw encoder contract;
pinned source identities are recorded in [upstream-sources.json](../upstream-sources.json).
