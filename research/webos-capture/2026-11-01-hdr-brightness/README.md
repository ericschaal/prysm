# HDR capture position and subdued LED brightness

Observed 2026-11-01 on the RTD2875Q TV, webOS 10.3.2-34, with PS5
HDR10/BT.2020 input and `hdrGame` picture mode. The user held The Witcher 3's
HDR settings preview still for the comparison.

## Capture is downstream of tone and gamut mapping

The isolated [native probe](capture-probe.cpp) acquired the shared capture lock
and captured three 320×180 NV12 frames at each tap, under an external watchdog:

| Capture option | Hardware input selector | Capture conversion |
| --- | --- | --- |
| Scaler output (1) | 3, second UZU | RGB→YUV bypassed |
| Display (2), Prysm default | 8, MEMC mux input | BT.709 limited RGB→YUV |
| Blended (3) | 4, OSD mixer | BT.709 limited RGB→YUV |

Tone mapping and gamut were changed one setting at a time with
`setSystemSettings`, `notify:true`, `store:false`. Original values were
`hdrDynamicToneMapping:on` and `colorGamut:auto`. Both runtime notifications and
saved settings were restored; the original post-gamut matrix and advancing
production capture were verified afterward. No vendor code, register or system
partition was written. Temporary probe and Luna role were removed.

Analysis uses a fixed game-preview region `[120,80,264,170]`, excluding menus.
Small temporal variation is bounded by the original/restored comparison.

| Comparison | Display-tap mean absolute difference, 8-bit code values |
| --- | ---: |
| Tone mapping on → off | Y 10.831; UV 1.354 |
| Original → restored | Y 0.159; UV 0.136 |
| Auto gamut → DCI-P3 | Y 0.417; UV 1.048 |

For the gamut comparison, all 19,440 Y/UV bytes in the earlier scaler tap's
preview region were identical, while 8,522 bytes changed at the display tap.
The reported post-gamut matrix changed from:

```
AUTO / 10000                 DCI-P3 / 10000
14802 -3899  -905            10812  -688  -122
 -615 10371   243               48  9769   183
  -81  -431 10512             -183  -595 10778
```

The display-RGB changes also agreed with the relative matrix transform
(channel-delta correlation about 0.93 for a simple encoded-RGB model). This
supports placement after gamut mapping; it does not calibrate the transfer
function. Native-vs-Auto was initially inconclusive because both retained the
same post-gamut matrix in this HDR mode. DCI-P3 supplied a distinct control.

[Capture statistics and hashes](capture-comparison.json), per-condition PQ
status, and original/restored settings retain the evidence. Raw captures remain
in temporary local storage; no game pixels or vendor binaries are committed.

## Conversion Prysm needs

Keep the verified limited-range BT.709 YUV→RGB decoding. The capture already
reflects TV rendering: do not apply another HDR tone mapper or a source
BT.2020→BT.709 primaries transform based on the HDMI metadata.

Matrix/range do not establish the optical primaries or transfer function of
post-gamut RGB. Prysm's sRGB linearization remains an approximation until that
response is measured. Placement after tone mapping alone does not establish
whether linearization requires sRGB, a power gamma, PQ, or a measured LUT;
linearization and tone mapping are distinct operations. Precise matching needs
a capture/display profile followed
by the LED-and-wall profile, preserving white point and hue. HDMI metadata alone
cannot supply that calibration. No speculative HDR conversion was added.

## Brightness finding

The live WLED controller runs 16.0.1. Its current limiter is disabled, brightness
scale is 100%, and observed master/segment brightness was 255. This agrees with
the user's report that maximum brightness is unrestricted. Realtime color gamma
is enabled at **2.8**; see [device settings](wled-observed.json).

Official WLED source traces DDP through `setRealtimePixel`, then `show()` through
`gamma32`, using `round(255 * (value/255)^gamma)`:
[DDP handling](https://github.com/wled/WLED/blob/v16.0.1/wled00/e131.cpp),
[output gamma](https://github.com/wled/WLED/blob/v16.0.1/wled00/FX_fcn.cpp),
[gamma table](https://github.com/wled/WLED/blob/v16.0.1/wled00/colors.cpp).

The [production processor loop](brightness-loop.rs) confirms uniform neutral
values survive processing and sRGB encoding at 100% brightness. Existing
sampler and UDP renderer regressions also pass. [Loop output](brightness-loop.txt):

| Input / Prysm DDP | Gamma 2.8 drive | Exact sRGB linear reference | Gamma 2.2 drive |
| ---: | ---: | ---: | ---: |
| 32 | 1 | 4 | 3 |
| 64 | 5 | 13 | 12 |
| 128 | 37 | 55 | 56 |
| 192 | 115 | 134 | 137 |
| 255 | 255 | 255 | 255 |

These are calculated 8-bit drive values, not measured light. The 2.8 curve
suppresses dark/middle channels relative to Prysm's sRGB model while preserving
maximum white. Gamma near 2.2 is a closer nominal sRGB baseline; realtime gamma
should remain enabled. WLED settings were inspected only, not changed.

Spatial averaging also reduces brightness/saturation around small highlights
and mixed colors by design. Uniform fixtures do not exhibit attenuation. Any
sampling change or shadow lift should be evaluated after the transfer response.
The active receiver used egui preview, brightness 100%, shadow lift 0 and
smoothing 0.1 s. No LED output was enabled or overridden.

## Limits

The experiment establishes relative capture placement in the current HDR Game
mode. Panel emission, strip spectra, reflected wall color, absolute luminance,
and exact post-tone RGB transfer/primaries remain unmeasured. No blanket claim
is made for other TVs, picture modes or alternate capture taps.
