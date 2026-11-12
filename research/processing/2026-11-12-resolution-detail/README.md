# Does a higher capture resolution add detail to the LEDs?

Date: 2026-11-12. Status: offline measurement; no product code changed; not run on the TV.
Platform: macOS, ffmpeg 9.0.2, cargo 1.99.0. Processor at `HEAD` = `56afb99` and the
uncommitted working-tree change to `crates/processing/prysm-processor` (bounded lattice
sampling). Settings: 162 LEDs (top 51, right 31, bottom 49, left 31), sample density 128,
spatial reach 10 %, black-bar removal on, smoothing off (and 0.1 s for one table).
Current pipeline: [architecture guide](../../../docs/developers/architecture.md).

## Finding

Hardly. On real content, the LED colours from a 1280×720 capture read in full differ from
those of a 640×360 box-filtered capture by at most 1–2 sRGB steps at the 95th percentile
and 3–4 steps at the 99th. 0.5 % of LED-frames differ by more than 5 steps, and 0.01 % by
more than 10. The largest differences, up to 16 steps, come from fine high-contrast
texture such as sunlit rock or foliage. A synthetic zone plate reaches 21 steps. The
cause is that the scaler averages gamma-encoded values, which darkens fine bright detail;
it is not a sharper picture. A finer cell grid does not separate neighbouring LEDs: a hard
edge spreads over the same number of LEDs at every grid size, because the spatial reach
sets that width.

The working-tree lattice (B) is closer to the full-detail reference than native 640×360
capture is. The A-vs-B differences are smaller than A-vs-C on every statistic. So with
the change, 1280×720 capture is not worse than 640×360, and it is slightly better.

A much larger difference has nothing to do with sampling detail. In dark scenes, the
black-bar detector makes different crop decisions at different capture sizes. In 204 of
720 frames of one dark excerpt, LEDs differ by up to 90–114 steps between 640×360 and
1280×720.

## Method

1. Each source becomes a 1920×1080, BT.709, limited-range 4:2:0 "master". The film is
   letterboxed into 1920×1080 as a TV shows it.
2. Each master is reduced with `scale=W:H:flags=area` to 1920×1080, 1280×720, 640×360 and
   320×180, and delivered as NV12 (limited-range BT.709), like the webOS capture tap. This
   assumes the TV's hardware scaler behaves like a box filter on gamma-encoded YUV. The
   real filter is unknown.
3. [`ledrun.rs`](ledrun.rs) feeds every frame, at 24 fps timestamps, through these
   processors:
   - the `HEAD` processor, which reads every pixel; its grid grows with size (128×72 at
     1280×720 and above);
   - the working-tree processor, which reads a lattice of every s-th pixel and row above
     640×360 (s = 2 at 1280×720, 3 at 1080p) on the 81×45 grid;
   - "E", a copy of the working-tree processor that reads every pixel into the capped
     81×45 grid. It separates the grid cap from point sampling, and it approximates
     averaging s×s blocks in linear light.

   Each processor runs with and without 0.1 s smoothing. The tool records the 162 LED
   colours exactly as the WLED renderer derives them (`color_at(i, count).to_srgb()`) and
   in linear light.
4. [`analyze.py`](analyze.py) compares the same frames: the absolute difference per
   channel in 8-bit sRGB steps over all LEDs and frames, and the share of LED-frames
   whose largest channel difference exceeds 2, 5 and 10 steps. A frame is counted as
   "crops agree" when both processors' normalized black-bar viewports match within 0.3 %.
   One row at 360 lines is 0.28 %, so this allows only rounding.

Comparisons: **A** = 1280×720 `HEAD` (full-detail reference), **B** = 1280×720 working
tree, **C** = 640×360 box-filtered capture (working tree = `HEAD` at this size; checked:
max 1 step), **D1080** = 1920×1080 `HEAD`, **D320** = 320×180 box-filtered capture.

Assumption, not a measured fact: on an LED strip, a just-noticeable difference is roughly
2–3 sRGB steps in mid-tones when two strips are compared side by side, and larger when
they are not.

### Content

| Source | Kind | Frames | Notes |
| --- | --- | --- | --- |
| `sintel-s1` | real (CGI film) | 720 | Excerpt from ≈88.5 s: bright city rooftops, interiors, motion |
| `sintel-s2` | real (CGI film) | 720 | ≈436 s: dark interiors, a face lit by flame, dark close-ups |
| `sintel-s3` | real (CGI film) | 720 | ≈491 s: dark blue cave, small bright crystals, near-black frames |
| `sintel-s4` | real (CGI film) | 720 | ≈710 s: sunlit textured rock, snowdrops, a bright sky |
| `goldengate` | real (camera aerial) | 480 | First 20 s of the local macOS wallpaper video `Golden Gate.mov` (4K HEVC, 10-bit, BT.709) |
| `testsrc2` | synthetic | 240 | ffmpeg test pattern: text, gradients, moving objects |
| `mandelbrot` | synthetic | 240 | Fractal with fine coloured filaments |
| `zoneplate` | synthetic | 240 | Moving pink/green zone plate, worst case for fine detail |
| `sweep` | synthetic | 240 | Red picture, with blue entering from the left edge at 192 px/s (1080p) |

Sintel: "Sintel" (2010), © Blender Foundation, durian.blender.org, licensed
[CC BY 3.0](https://creativecommons.org/licenses/by/3.0/), from
`https://download.blender.org/durian/movies/Sintel.2010.1080p.mkv` (1920×818, H.264,
BT.709 limited range). Four 30 s excerpts were copied without re-encoding, using `-ss`
95, 440, 500 and 715 s. Each copy starts at the keyframe at or before that time; the
approximate starts are listed above. The English subtitle track was kept but not
rendered, because this ffmpeg build has neither libass nor drawtext. Subtitles therefore
remain untested. The Golden Gate clip is an Apple system asset: it is not redistributable,
and it is only available on macOS. `run.sh` skips it when it is absent. No video is
committed.

## Results

All tables come from [`results.md`](results.md), which `analyze.py` generates (columns per
channel: max / mean / p95 / p99 in sRGB steps).

### Real content, frames whose black-bar crops agree, no smoothing

| Comparison | R | G | B | > 2 | > 5 | > 10 |
| --- | --- | --- | --- | --- | --- | --- |
| **A vs C** (does 1280 add detail over 640?) | 16 / 0.49 / 2 / 4 | 15 / 0.39 / 2 / 4 | 12 / 0.36 / 1 / 3 | 4.16 % | 0.51 % | 0.01 % |
| **A vs B** (cost of the lattice and grid cap) | 10 / 0.33 / 1 / 3 | 11 / 0.29 / 1 / 2 | 11 / 0.28 / 1 / 2 | 1.42 % | 0.04 % | 0.00 % (1) |
| **B vs C** (1280 lattice vs 640) | 15 / 0.55 / 2 / 4 | 14 / 0.47 / 2 / 4 | 10 / 0.43 / 2 / 3 | 3.65 % | 0.37 % | 0.00 % (19) |
| D1080 vs A | 6 / 0.54 / 3 / 3 | 6 / 0.43 / 2 / 3 | 6 / 0.36 / 2 / 3 | 6.58 % | 0.00 % | 0 |
| D1080 vs C | 20 / 0.94 / 4 / 7 | 20 / 0.73 / 3 / 6 | 15 / 0.61 / 2 / 5 | 13.85 % | 1.87 % | 0.13 % |
| D1080 vs 1080 lattice | 12 / 0.29 / 1 / 3 | 13 / 0.24 / 1 / 3 | 13 / 0.21 / 1 / 2 | 2.03 % | 0.12 % | 0.00 % |
| C vs D320 | 28 / 1.12 / 4 / 7 | 31 / 0.93 / 4 / 6 | 24 / 0.86 / 3 / 5 | 16.95 % | 2.02 % | 0.18 % |
| A vs E (grid cap only) | 8 / 0.20 / 1 / 2 | 9 / 0.16 / 1 / 2 | 8 / 0.14 / 1 / 1 | 0.78 % | 0.01 % | 0 |
| E vs B (point lattice only) | 5 / 0.23 / 1 / 2 | 6 / 0.22 / 1 / 2 | 5 / 0.22 / 1 / 2 | 0.13 % | 0.00 % (1) | 0 |
| E vs C (640 scaler only) | 16 / 0.48 / 2 / 4 | 13 / 0.37 / 2 / 4 | 10 / 0.32 / 1 / 3 | 3.59 % | 0.34 % | 0.01 % |

Frame counts: 3,360 frames × 162 LEDs, or 3,118 frames where crops at 640×360 and
1280×720 disagree and are excluded. The mean signed difference is +0.09 steps for A − C
and +0.42 for D1080 − A: higher capture sizes come out slightly brighter. In linear light,
the A-vs-C maximum is 0.075 and the mean is 0.001.

With 0.1 s smoothing (what reaches the LEDs), the figures barely change. A vs C: p95
2/2/1, p99 4/4/3, max 16, with 4.04 % > 2, 0.46 % > 5 and 0.01 % > 10. A vs B: max 9,
with 1.31 % > 2 and 0.02 % > 5. B vs C: max 17, with 3.56 % > 2 and 0.33 % > 5.

**Where the A-vs-C differences arise.** With crops agreeing, the largest differences (14–16
steps) are on `sintel-s4`'s sunlit rock texture and snowdrops (the left, bottom-right and
right strips), on `sintel-s1` rooftops, and on `sintel-s2` lit interiors. In each case
A is brighter than C, which matches gamma-space averaging of fine bright detail. The
camera aerial (`goldengate`) stays within 6 steps. By source, the maximum is: goldengate
6, s1 15, s2 14, s3 12, s4 16. The A-vs-B maxima (9–11 steps) are isolated LEDs on the
right strip of `sintel-s3`/`s4`, where the point lattice catches or misses fine
highlights.

### Black-bar crops differ with capture size

| Source | Frames whose crop differs, 640 vs 1280 (and 640 vs 1080) | Largest LED difference in those frames |
| --- | --- | --- |
| `sintel-s3` (dark cave) | 204 / 720 | 90 steps (top strip) |
| `sintel-s2` (dark interiors) | 38 / 720 | 114 steps (bottom strip, frames 122–123) |
| all other sources | 0 | — |

In these frames, the 640×360 detector crops a bar, for example a top-only crop or a
3-pixel side crop, that the 1280×720 and 1920×1080 detectors do not confirm, or the
reverse. Including these frames raises A vs C to 114 / 90 / 89 steps maximum and 1.19 %
of LED-frames > 10. `HEAD` and the working tree crop identically at the same size (A vs B:
0 differing frames). This is a detector behaviour that depends on resolution, and it is
the largest effect measured here.

### Synthetic content, no smoothing (no crop differences)

| Comparison | R | G | B | > 2 | > 5 | > 10 |
| --- | --- | --- | --- | --- | --- | --- |
| A vs C | 20 / 5.66 / 17 / 18 | 11 / 0.45 / 2 / 5 | 21 / 5.60 / 18 / 19 | 42.85 % | 34.72 % | 33.35 % |
| A vs B | 6 / 0.41 / 1 / 3 | 7 / 0.17 / 1 / 2 | 8 / 0.35 / 1 / 2 | 1.89 % | 0.04 % | 0 |
| B vs C | 21 / 5.65 / 17 / 18 | 10 / 0.46 / 2 / 5 | 21 / 5.55 / 18 / 18 | 41.38 % | 34.46 % | 33.34 % |
| D1080 vs A | 22 / 6.61 / 21 / 21 | 5 / 0.19 / 1 / 2 | 22 / 6.64 / 21 / 22 | 37.73 % | 33.49 % | 33.33 % |

The zone plate accounts for the > 10 share: every LED in every frame, 17–22 steps on red and
blue, with the full-resolution average brighter (mean signed A − C +3.6). Without it,
`testsrc2` reaches a maximum of 15 steps and `mandelbrot` 12, for A vs C. Each further
gamma-space reduction (1080 → 720 → 360) loses about the same again. This sets the upper
bound for the effect: only extreme, screen-filling fine detail produces visible
differences.

### Neighbouring-LED separation: a hard edge on the top strip

| Case | Cell grid | 25–75 % transition width (LEDs) | Largest step between neighbouring LEDs | LEDs between 10 and 90 % |
| --- | --- | --- | --- | --- |
| D1080 `HEAD` | 128×72 | 2.94 | 0.271 | 18.01 |
| A 1280 `HEAD` | 128×72 | 2.94 | 0.270 | 17.98 |
| E 1280 every pixel | 81×45 | 2.98 | 0.263 | 18.06 |
| B 1280 lattice | 81×45 | 2.98 | 0.263 | 18.03 |
| C 640 box | 81×45 | 3.02 | 0.255 | 17.95 |
| D320 box | 64×36 | 3.32 | 0.220 | 18.25 |

The ratio is r / (r + b) in linear light, measured where the edge lies in the middle 60 % of
the top strip. The finer grid narrows the transition by 0.04 LED. The sampler's spatial
reach (radius 10 % of the picture height, about 2.9 LEDs on the top strip) and its broad
tail set the width at every size. Only 320×180 is measurably softer, by 0.3 LED.

## Conclusion

Above 640×360, capture resolution adds almost nothing to what the LEDs show. Typical
differences are 1–2 steps, the 99th percentile is 3–4 steps, and the worst cases on real
fine texture are 16 steps, with A brighter. The finer grid does not separate neighbouring
LEDs any better. What resolution does change is a small brightness bias in very fine
high-contrast detail, because the TV's scaler averages gamma-encoded values. The
working-tree lattice keeps most of that benefit (B is closer to A than C is), at the cost
of up to 11 steps on isolated LEDs. The visible difference in this data comes from the
black-bar detector cropping differently at different sizes in dark scenes, not from
sampling detail.

Limits: the TV's real scaler filter is unknown, and was modelled as a box filter on
gamma-encoded 4:2:0. Sharper polyphase filters would move C toward point sampling. The
content is a CGI film and one camera aerial, 140 seconds in total, without rendered
subtitles. The source of truth was 1080p; no 4K film was tested. The just-noticeable
difference figure is an assumption. Smoothing, colour correction and WLED transport were
tested only as described, and nothing was verified on the TV or the LEDs.

## Reproduce

```sh
WORK=/path/to/scratch sh research/processing/2026-11-12-resolution-detail/run.sh
```

[`run.sh`](run.sh) builds a renamed copy of `HEAD`'s processor (`git archive`), an
every-pixel copy of the working-tree processor, and `ledrun` against the repository's
`prysm-core` and working-tree `prysm-processor`, all under `$WORK`. It then downloads the
four Sintel excerpts (about 170 MB) and runs every source at every size. It prints the
tables that are saved as [`results.md`](results.md). A full run takes about 15 minutes on
an Apple-silicon Mac. Set `HEAD_REV` to compare a different baseline. Intermediate files
(LED records and the excerpts) stay in `$WORK`.
