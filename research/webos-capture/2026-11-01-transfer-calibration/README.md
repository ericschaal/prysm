# Capture-to-light calibration — measurement in progress

The user has a Sony A7 III. Original uncompressed RAW photographs can provide a
relative capture-code→light measurement and a camera-based TV/LED match. No RAW
photographs have been supplied yet; no transfer curve or calibration is claimed.

## Digital observations

Earlier controlled tests establish that the default Display capture tap follows
HDR tone mapping and post-gamut processing in the current HDR Game mode. This
does not identify its optical transfer function.

The exact TV picture-controller executable was copied for offline inspection.
Its exported `gGammaLUT_R/G/B` arrays and two named gamma-enable flags were read
from the running process using read-only `/proc/<pid>/mem`, restricted to those
named arrays/flags. No debugger was attached and no memory/register was written.
[Observation and executable hash](digital-gamma-observation.json):

| Controller LUT | Endpoint / 1023 | Normalized fitted exponent | Linear-fit RMSE, code values |
| --- | ---: | ---: | ---: |
| R | 1007 | 1.00029 | 0.289 |
| G | 972 | 0.99986 | 0.365 |
| B | 1023 | 1.00000 | 0.000 |

These cached engine tables are effectively linear channel gains. They do not
reveal the entire active HDR pipeline or the panel's code→light response. The
named `gamma22En` and `gamma045En` flags were both zero; that must not be
interpreted as proof of linear emitted light or proof that every gamma block is
bypassed. Other HDR/gamut transfer blocks and the panel remain in the chain.

The generic picture-controller gamma metadata request does not expose an active
transfer curve. A low-level HDR LUT getter also changes hardware LUT read/write
controls internally, so it was not invoked merely to obtain a speculative curve.

## First optical measurement

1. Keep the same static HDR game settings preview and picture mode. Include
   varied dark/middle/bright areas; a white-only clipping screen is insufficient.
2. Put the A7 III on a tripod, nearly square to the whole TV. Keep the room dark
   and LEDs off for this stage. Focus once, then use manual focus.
3. Use original uncompressed RAW, single shot, mechanical shutter, manual ISO
   100/exposure and fixed white balance. Start at 1/15 s for the observed 60 Hz
   signal; adjust aperture to preserve bright detail, then lock exposure. Avoid
   clipped channels and visible refresh bands. Do not change exposure between
   the first shot and a repeat shot.
4. Once the camera is ready, acquire a short burst of matched digital capture
   frames, then the RAW photograph(s). Compare temporal variation and exclude
   moving image regions. Retain the original ARW, not an exported JPEG/preview.

The A7 III supports 14-bit RAW and uncompressed RAW:
[Sony file format](https://helpguide.sony.net/ilc/1720/v1/en/contents/TP0001667676.html),
[RAW type](https://helpguide.sony.net/ilc/1720/v1/en/contents/TP0001661967.html).

## Analysis and acceptance

- Decode sensor samples linearly, subtract recorded black levels, and reject
  clipped/noisy regions. Do not fit against a camera JPEG tone curve.
- Register the photographed TV to the matched capture. Compare region averages
  at adequate scale to suppress moiré, pixel structure, and registration errors.
- Fit sRGB, power-gamma and normalized PQ candidates with exposure/channel
  mixing treated separately. Use held-out regions, not fitting error alone.
- Inspect residuals by brightness, hue and position. If local dimming, glare,
  vignetting, or content dependence prevents one global curve from explaining
  the data, report that instead of assigning a misleading gamma number.
- If the source image lacks tonal coverage or stability, replace it with a
  controlled chart before drawing a transfer-function conclusion.

After the capture response is established, photograph neutral ramps and RGB/
secondary LED patches reflected from the actual wall, at fixed exposure within
each sequence. Fit channel response and white balance first, then the smallest
color transform supported by held-out patches; reduce out-of-gamut saturation
without arbitrary per-channel clipping. Relative brightness is tuned afterward.

The camera can optimize a relative match in its sensor space. Spectral mismatch
between the LCD/OLED, LEDs, camera and human vision prevents claiming absolute
colorimetric accuracy without a suitable reference instrument. No arbitrary
tone mapper, gain or saturation adjustment is implemented before measurement.
