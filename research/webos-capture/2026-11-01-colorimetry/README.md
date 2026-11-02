# Capture colorimetry on the TV

Observed 2026-11-01 on webOS 25 / Rockhopper 10.3.2-34, Realtek RTD2875Q,
kernel 5.4.268-294.24.papikonda.2. The default **Display (2) capture tap emits
limited-range BT.709 NV12**, even when capture is scaled to 320×180 or 640×360.
The previous BT.601 interpretation was incorrect for this path.

## Evidence

1. `com.webos.service.videooutput/getStatus` reports the active 3840×2160/50 Hz
   HDMI source as `colormetry: ITU709`, `ycbcrRange: LIMITED`,
   `pixelEncoding: YCbCr420`, `hdrType: none`. HDMI infoframe diagnostics in
   `/proc/lgtv-driver/hdmi_status/hdmi.p0` independently report colorimetry 2
   and YCC quantization 0. This establishes the input, not the capture output.
2. An isolated capture probe acquired the shared capture lock, initialized the
   normal display tap, captured one frame, queried `VIDIOC_G_FMT`, and completed
   stop/postprocess/finalize/release. The getter reports geometry but leaves
   colorspace and encoding fields unspecified. Pixel extrema were Y=19–219;
   those extrema alone cannot establish range. See [probe output](capture-format.stderr)
   and [source](capture-probe.cpp), adapted from the existing lifecycle probe.
3. A read-only copy of `/boot/Image.lz4-5.4.268-294.24.papikonda.2` establishes
   the actual firmware conversion, rather than assuming another TV's driver.
   The ARM64 Image header's text offset is `0x180000`; symbol addresses come
   from `/proc/kallsyms`. At `drvif_color_colorspaceyuv2rgbtransfer`
   (`0xffffffc010a95584`), NV12/NV16 select the coefficient table at
   `0xffffffc01182b028`. See [disassembly](capture-conversion.asm).
4. While normal capture was running, a **read-only** `/dev/mem` mapping read
   seven configuration registers at physical `0x18029c00`–`0x18029c18`.
   They match that firmware table exactly. Control `0xc0080051` selects
   RGB→YUV, enables chroma offset, and does not bypass the matrix.

The signed matrix coefficients, divided by 4096, are:

| Output | R | G | B |
| --- | ---: | ---: | ---: |
| Y | 0.183105 | 0.614014 | 0.062012 |
| Cb | -0.101074 | -0.337891 | 0.438965 |
| Cr | 0.438965 | -0.398926 | -0.040039 |

These are quantized limited-range BT.709 coefficients: luma weights correspond
to 0.2126/0.7152/0.0722 scaled by 219/255, with black offset 16 and chroma
center 128. The programmed luma-offset fields are `0x100`, representing 16
at the hardware's fixed-point precision. This differs substantially from
BT.601's 0.299/0.587/0.114 luma weights.

[Structured evidence](evidence.json) contains the exact coefficients, register
values, firmware identity and image SHA-256 hashes. No captured pixels or vendor
binaries are stored here. Installed capture was briefly stopped for the probe,
then restored and verified capturing/connected without changing saved settings,
TV input, picture settings, power, or any system partition. Temporary probe and
Luna role/manifest were removed.

The public [Realtek capture driver](https://github.com/TCLOpenSource/rt2875p/blob/main/drivers/rtk_kdriver/tvscalercontrol/rtd6748/scaler_vtdev.c)
helped identify the conversion register layout and control flags. Its table names
are misleading and its constants differ from this firmware; it was not used as
proof of the TV's active matrix.

## Application correction

- Decode native NV12/NV16 as limited-range BT.709 before RGB UI composition.
- Encode composed NV12 and H.264 I420 with the same matrix/range.
- Signal BT.709 limited range in H.264 VUI; reject contradictory SPS metadata.
- Decode Raw/LZ4 YUV and H.264 with BT.709 limited range.
- Keep RGB full range and use balanced conversion precision throughout.
- Use handshake/frame protocol version 3. Update sender and receiver together;
  version 2's fixed BT.601 contract is rejected rather than silently reinterpreted.

Regression tests cover native primary colors and black/white through all three
codecs, RGB composition, H.264 color/shadow patches, mismatched SPS metadata,
legacy protocol rejection, frame skipping, and reconnect state.

Validation: 85 protocol/capture/sender tests and two application webOS lifecycle
tests passed. Clippy passed for the three capture/codec crates with all targets
and warnings denied. Changed Rust files passed rustfmt; the loopback receiver
passed Node syntax checking. The corrected sender cross-built for ARMv7 with the
webOS SDK. A workspace-wide
format check still reports an unrelated formatting difference in `prysm-api`.

After the user authorized deployment, version 0.1.11 was installed on the TV.
Both installed executables matched the build's SHA-256 hashes. The active desktop
receiver already matched the current protocol-3 release build, verified by a
fresh Cargo build and its mapped executable. Saved TV/receiver configuration,
autostart, enabled state, boot hook, and capture permission mount were preserved.
The receiver returned to running with no pipeline or TV errors; five consecutive
samples advanced from 723 to 850 processed frames. Metrics confirmed 640×360
H.264 payloads and successful decoder work. See [deployment evidence](deployment.json).
HDR, long-duration thermal behavior, standby/wake, and cold boot were not retested.

## Limits

This verifies the default display capture path on this firmware and the observed
SDR HDMI source. It does not calibrate alternate capture taps, other TV models,
HDR transfer functions, panel gamut, or LED output. Native buffers/API headers do
not supply per-frame colorimetry; supporting other capture domains needs verified
metadata or an explicit conversion before this fixed wire contract.
