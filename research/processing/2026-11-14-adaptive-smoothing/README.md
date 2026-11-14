# Smoothing with scene cuts and a brightness ramp: lag against shimmer

Date: 2026-11-14. Status: offline measurement of the smoothing knobs; not run on the
TV or the LEDs. Platform: macOS, cargo 1.99.0. Processor at `HEAD` = `678e3fe` plus the
uncommitted working-tree change that replaces the first design (below) with **Scene
cuts** and **Brightness ramp** (`crates/processor/src/nodes/temporal_smoother.rs`).
Settings: 162 LEDs (top 51, right 31, bottom 49, left 31), sample density 128, spatial
reach 10 %, black-bar removal off. User guide:
[Picture settings](../../../docs/users/configuration.md#processing).

## Finding

With smoothing at 0.5 s and Scene cuts at 100 %, a hard cut is done in the cut frame
for every LED (today's 0.1 s default: 67 ms), while grain moves the LEDs about a quarter
as much as today's default does (dark, blotchy grain: mean 0.04 against 0.17 steps per
frame at 30 fps). Scene cuts change nothing else: fades, pans, grain and a block moving
along one edge are smoothed exactly as by a fixed 0.5 s, because the cut detector fired
on none of their frames, at 30 or 60 fps. It fired on every real cut, at full strength.

The brightness ramp holds a dark-to-bright change to its time: the bright cut takes
233 ms (ramp 0.5 s) or 467 ms (ramp 1 s) at any Scene cuts setting, and a one-frame
explosion shows 13 % (0.5 s) or 7 % (1 s) of the way at 30 fps instead of all of it.
The ramp is a second stage: the lights follow the smoothed colour as fast as the ramp
allows. So with Scene cuts at 100 % an ordinary cut (sunset to forest), whose brightness
changes little, still lands within 33 ms (ramp 0.5 s) or 67 ms (ramp 1 s) for the
median LED, at 30 and at 60 fps alike.

None of the grain here reaches one just-noticeable step per frame at the LEDs, even
unsmoothed (0.2–0.3 mean, 0.5–0.7 at the 95th percentile): three code values of grain
average away inside each LED's region. The shimmer columns therefore compare settings,
not whether shimmer is visible. Whether flicker of a fraction of a step is seen on a
wall, and which settings look best, only the TV can tell.

## The design

Each frame, every LED sample moves from what the lights show toward its new colour,
blended in linear light, by a share of the way:

1. **Smoothing** (`smoothing_seconds`): ordinary change moves
   `share = 1 - e^(-ln 20 · Δt / seconds)`, 95 % in the smoothing time, independent of
   the frame rate. This is the fixed rate Prysm had before.
2. **Scene cuts** (`cut_response_percent`): a cut is detected from the input, not from
   the lights. A sample has *changed* when its Oklab distance from the previous input
   exceeds 3 steps (0.06); samples darker than Oklab lightness 0.1 in both inputs are
   left out, so black bars do not hide a cut. The cut strength `c` rises smoothly
   (smoothstep) from 0 at 40 % of lit samples changed to 1 at 70 %. On a cut each sample
   first moves `response · c` of the way, then the ordinary share:
   `1 - (1 - response · c) · (1 - share)`. A pan or a fade moves a sample well under a
   step per frame, and a block moving along one edge changes at most about a third of
   the samples, so neither is a cut; a cut changes nearly all.
3. **Brightness ramp** (`brightness_ramp_seconds`): perceived lightness, the cube root
   of Rec. 709 luminance `Y`, may change by at most `Δt / ramp` per frame. `Y` is
   linear along a blend, so the largest allowed share is closed-form,
   `(Y' - Y_shown) / (Y_wanted - Y_shown)` with `Y' = (∛Y_shown ± Δt / ramp)³`. It is a
   second stage: 1 and 2 give the colour the lights should have, and the lights follow
   it by at most that share. Equal luminance (a change of hue alone) is not capped.

Every result is therefore a mix of the shown colour and the wanted one, itself a mix of
inputs: no channel leaves the range between them, so nothing overshoots or leaves the gamut. The unit tests check
this on the emitted light for a grid of all three knobs.

### The first design, replaced

The first version of this study (same date) made the rate grow with the Oklab distance
between shown and target colour, `k(d) = k0 · (1 + β · d / JND)`, and offered three
ways for brightness to follow (with colour, faster than colour, limited). On the TV it
was wrong in two ways. Faster and Limited blended Oklab lightness and hue at different
rates, which left the sRGB gamut: one linear channel reached 1.6× (Faster) and 2.5×
(Limited) of full, a visible over-bright flash. Its no-overshoot test had checked the
Oklab components, not the light. And because the rate grew from zero with the error,
everything visible moved at about the default speed and only grain below one step was
smoothed, so the knobs overlapped and none had a distinct effect; the error between
shown and target cannot tell a cut from ordinary motion. The input can, which is what
Scene cuts uses.

## Method

[`smoothrun.rs`](smoothrun.rs) builds synthetic 320×180 RGB24 pictures, feeds them to the
real `PrysmProcessor` at exact 30 and 60 fps timestamps, and reads the 162 LEDs as the
WLED output does. The cut detector column calls the processor's own `cut_strength` on
consecutive unsmoothed inputs. Distances are Oklab, in steps of 0.02 (a just-noticeable
difference). Nothing is downloaded; the noise is seeded, so every run is identical.

| Sequence | What | Measure |
| --- | --- | --- |
| Cut | 1 s warm sunset, then 2 s cool forest | Time from the cut frame until each LED is within 10 % of its starting distance to the forest (90 % done); median and worst LED, LEDs that change by at least one step |
| Bright cut | 1 s dim room at night, then 2 s screen-filling explosion | As for the cut |
| One-frame flash | The dim room with the explosion on one frame at 1 s | How much of the way to the explosion the median LED shows on its frame; time until every LED is back within one step of the room |
| Grain, bright fine | 5 s still sunset, ±3 code values per 8×8 block per frame (mostly luma) | Frame-to-frame LED change after the first second, mean and 95th percentile |
| Grain, bright blotchy | As above with 32×32 blocks (the blotchy noise of flat compressed areas) | As above |
| Grain, dark blotchy | The dim room with 32×32 blocks | As above |
| Fade | 0.5 s sunset, a 3 s cross-fade in code values to the forest, 0.5 s forest | During the fade: distance to the unsmoothed LEDs (mean, max) and the largest frame-to-frame change |
| Pan | A smooth hue panorama moving half a screen width per second, 4 s | After the first second: as for the fade |
| Local motion | The forest with a red block a fifth of the screen wide moving along the top edge and back, 4 s | Distance to the unsmoothed LEDs |

Every sequence also goes through the cut detector: on how many frame changes it fires
(strength above 0) and its largest strength. It reads the input only, so this is the
same for every setting.

Lag is measured in whole frames: 0 ms means the cut frame itself is already 90 % done.

## Results

Generated by `run.sh` into [`results.md`](results.md); the 30 fps tables are copied here.
"Behind" and "largest step" are in steps; the unsmoothed row shows the picture's own
largest step.

### Transitions, 30 fps

| Setting | Cut: 90 % done, median / worst LED (ms) | Bright cut: 90 % done, median / worst (ms) | One-frame flash: shown (% of the way, median) / back within 1 step (ms) |
| --- | --- | --- | --- |
| off (raw) | 0 / 0 | 0 / 0 | 100 / 33 |
| **today: 0.1 s, cuts 0 %** | 67 / 67 | 33 / 33 | 82 / 200 |
| 0.5 s, cuts 0 % | 367 / 467 | 233 / 233 | 45 / 667 |
| 0.5 s, cuts 50 % | 233 / 333 | 133 / 133 | 79 / 767 |
| 0.5 s, cuts 100 % | 0 / 0 | 0 / 0 | 100 / 33 |
| 0.5 s, cuts 0 %, ramp 0.5 s | 367 / 467 | 233 / 267 | 13 / 667 |
| 0.5 s, cuts 50 %, ramp 0.5 s | 233 / 333 | 233 / 267 | 13 / 767 |
| 0.5 s, cuts 100 %, ramp 0.5 s | 33 / 100 | 233 / 267 | 13 / 33 |
| 0.5 s, cuts 0 %, ramp 1 s | 367 / 467 | 467 / 567 | 7 / 667 |
| 0.5 s, cuts 50 %, ramp 1 s | 233 / 333 | 467 / 567 | 7 / 767 |
| 0.5 s, cuts 100 %, ramp 1 s | 67 / 200 | 467 / 567 | 7 / 33 |

### Motion, 30 fps: behind the unsmoothed LEDs (steps)

| Setting | Fade: behind, mean / max | Fade: largest step | Pan: behind, mean / max | Pan: largest step | Local motion: behind, mean / max |
| --- | --- | --- | --- | --- | --- |
| off (raw) | 0.00 / 0.00 | 0.24 | 0.00 / 0.00 | 0.47 | 0.00 / 0.00 |
| **today: 0.1 s, cuts 0 %** | 0.05 / 0.12 | 0.21 | 0.18 / 0.27 | 0.46 | 0.06 / 2.70 |
| 0.5 s, any cuts, any ramp | 0.40 / 0.77 | 0.18 | 1.34 / 1.95 | 0.43 | 0.38 / 6.86 |

### Grain, 30 fps: frame-to-frame LED change, mean / p95 (steps)

| Setting | Bright, fine | Bright, blotchy | Dark, blotchy |
| --- | --- | --- | --- |
| off (raw) | 0.10 / 0.23 | 0.20 / 0.46 | 0.31 / 0.69 |
| **today: 0.1 s, cuts 0 %** | 0.05 / 0.12 | 0.11 / 0.25 | 0.17 / 0.37 |
| 0.5 s, any cuts, any ramp | 0.01 / 0.03 | 0.03 / 0.06 | 0.04 / 0.09 |

All nine 0.5 s rows are identical in the motion and grain tables (see
[`results.md`](results.md)): the detector never fired there, and none of these
sequences changes brightness fast enough to meet the ramp.

### Cut detector, 30 fps: false cuts

| Sequence | Frame changes | Cuts in it | Detector fired | Largest strength |
| --- | --- | --- | --- | --- |
| Cut | 89 | 1 | 1 | 1.00 |
| Bright cut | 89 | 1 | 1 | 1.00 |
| One-frame flash | 89 | 2 | 2 | 1.00 |
| Pan | 119 | 0 | 0 | 0.00 |
| Fade | 119 | 0 | 0 | 0.00 |
| Grain, bright fine | 149 | 0 | 0 | 0.00 |
| Grain, bright blotchy | 149 | 0 | 0 | 0.00 |
| Grain, dark blotchy | 149 | 0 | 0 | 0.00 |
| Local motion | 119 | 0 | 0 | 0.00 |

At 60 fps ([`results.md`](results.md)) the detector again fires on the cuts only, and
every smoothed row shimmers less per frame, because each frame covers half the time.
The lags agree to within a frame or two, with or without a ramp. The one-frame flash
shows less at 60 fps under a ramp, which is right: the flash itself is half as long.

**Reading the flash rows.** Scene cuts at 50 % takes half of the flash at once, and half
of the cut back, so the lights then ease back from a brighter point than with a fixed
rate: "back" is longer (767 ms) than at 0 % (667 ms). At 100 % the flash shows on its
frame and is gone on the next, as unsmoothed. A ramp holds the flash to a few percent
on the lights; how long they take to come back follows the smoothed colour underneath,
so it is as without the ramp.

## What the numbers cannot show

- The pictures are synthetic and the grain is a model (uniform block offsets); real film
  grain, compression noise and the TV's own processing were not measured. Real cuts
  between similar shots (two angles of one room) change fewer samples and get a partial
  strength; how often that happens in films was not measured.
- The detector thresholds (3 steps, 40 % and 70 %) leave a wide margin on these
  sequences, but a fast pan at a low frame rate, or a large object filling most of the
  screen, could look like a cut; neither was measured.
- One step (0.02 in Oklab) is a side-by-side figure for colour patches; how visible a
  fraction of a step of flicker is on a wall behind a TV, in the dark, is not known.
- Whether a fast cut or a calm one looks better, and which ramp is pleasant during an
  explosion, is a matter of taste on the real TV.
- WLED's own transition and the LEDs' response were not part of this.

## Reproduce

```sh
WORK=/path/to/scratch sh research/processing/2026-11-14-adaptive-smoothing/run.sh \
  > research/processing/2026-11-14-adaptive-smoothing/results.md
```
