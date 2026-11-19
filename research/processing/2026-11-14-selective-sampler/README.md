# Colour focus: a selective spatial sampler, its accuracy, cost and flicker

Date: 2026-11-14. Status: offline measurement of a new Picture setting, **Colour focus**
(`color_focus_percent`, default 0, off); not yet run on the TV or the LEDs. Platform:
macOS on an Apple M5 Pro, one thread, cargo 1.99.0. Processor at `HEAD` = `71cbfc7` plus
the uncommitted working-tree change that adds
[`color_focus.rs`](../../../crates/processor/src/nodes/spatial_sampler/color_focus.rs).
Settings: sample density 128, spatial reach (Edge depth) 10 % unless stated, black-bar
removal off. User guide: [Picture settings](../../../docs/users/configuration.md#processing).
Background: [how much colour the plain average loses](../2026-11-14-mean-colourfulness/README.md).

## Finding

Colour focus does what the colourfulness model promised on pictures made for it: beside
a boundary between orange and blue, the LEDs' chroma rises from 0.168 to 0.200 (the
halves themselves have 0.196 and 0.222), and an LED looking at a green object on grey
keeps the object's hue. At 0 the output is bit for bit the plain average.

The fast two-part evaluation stays within a just-noticeable step of the same formula
evaluated on every cell for every LED: 95th percentile at most 0.62 steps on every test
picture at every edge depth from 1 % to 100 % (focus 100), the worst being seeded random
blocks of 16 pixels. On this Mac it adds 0.14–0.15 ms per frame at the default edge
depth to a processor that takes 0.65 ms, and 0.35–0.39 ms at an edge depth of 100 %, the
most expensive. On the TV it costs far more than that ratio suggests: measured with
`bench` on the dev TV, 7.8 ms per frame at 10 % and 9.6 ms at 100 %, on a processor that
takes 11.3 ms without it. At 30 fps that fits one thread (19 ms of 33); at 60 fps
(16.7 ms per frame) it does not.

Focus does not make grain flicker more. With the grain of the smoothing study, the
frame-to-frame change of the LEDs beside a boundary rises by at most 11 % at focus 100,
from 0.07 to 0.08 steps per frame. A boundary that moves *at* an LED is followed more
sharply: a one-pixel shift there moves that LED 1.64 steps at focus 100 against 0.52 at
focus 0, 3.2 times as much. The formula itself does that, since evaluating it on every
cell gives the same, and the unit tests bound it at 4 times.

## The pass

For each LED sample, with strength `s` = focus / 100:

1. **Anchor**: the colour next to the LED, a mean over the kernel's local lobe
   (`64/(1+16d²)²`) alone with grey counted less (weight × `1 + chroma / 0.1`), as Oklab
   `a, b`.
2. **Selective mean**: every cell weighted by its geometric weight ×
   `1 - s + s · agreement · (1 + chroma / 0.1)`, where
   `agreement = 1 / (1 + |ab - ab₀|² / 0.08²)` compares hue and saturation only.
3. **Brightness**: the selective colour is scaled to the plain mean's Rec. 709
   luminance, but no channel above full; a selective colour with no light gives the
   plain mean.

σ = 0.08 and the grey factor are those of the colourfulness model. The plain mean is
computed as before, unchanged, and is also what the brightness follows.

### The two-part evaluation

Per frame, each cell's colour (3,645 cells at 640×360) is pooled twice: into
**near groups** and into **coarse blocks**. Per geometry each LED keeps a sparse list of
the near groups within its **near field** and one weight per block for every other cell,
so no cell counts twice. Per frame, each LED evaluates the agreement for its near list
and for every block.

| Size | Rule (constant) | At 640×360, Edge depth 10 % |
| --- | --- | --- |
| Near field | 0.2 of the picture height (`NEAR_HEIGHT`), but between 1 and 2 kernel radii (`NEAR_RADII_MIN`, `NEAR_RADII_MAX`) and at least 1.5 block widths (`NEAR_MIN_BLOCKS`) | 72 px, two radii; about 120 cells per LED |
| Near groups | at most half the local lobe's width (`GROUP_LOBES`) and at most 2 cells (`GROUP_CELLS`) | single cells (8 px); pairs from an edge depth of 35 % |
| Coarse blocks | about 100 for the picture (`POOLED_BLOCKS`), at least 3 near groups wide (`BLOCK_GROUPS`) | 112 blocks of 6×6 cells (47 px) |

The brief started from two radii and about 400 blocks. Two things changed with
measurement:

- **The sizes follow the kernel.** With a fixed two-radius near field of single cells
  the cost grew with the square of the edge depth: 0.66 ms at 30 % and 2.1 ms at 100 %
  on this Mac, against 0.22 ms at 10 %. Grouping the near field by the kernel alone
  (groups of a whole local-lobe width) bounded the cost but missed the accuracy target
  from 20 % on (p95 1.1–3.2 steps), because random 16-pixel detail does not grow with
  the kernel. Hence a near field bounded by the picture height and groups of at most
  two cells.
- **100 blocks, not 400.** At the default edge depth the 95th percentile is 0.62
  steps with 100 blocks and 0.38 with 400; both pass, and 100 halves the per-LED work
  (234 colours per LED instead of 527). The pass costs 0.14 ms instead of 0.23 ms.

The pass skips groups that no LED reads, so their Oklab is never computed. All storage
is kept from frame to frame. A new geometry (crop, capture size, density or edge depth)
rebuilds the lists on its first frame, as the plain weights already are rebuilt.

## Results

Generated by [`run.sh`](run.sh) into [`results.md`](results.md). Steps are Oklab
distances of 0.02 (one just-noticeable difference).

### Accuracy against every cell, Edge depth 10 %

Per LED: Oklab distance between the two-part evaluation and the same formula evaluated
on every cell (anchor included) for every sample, 254 samples at 640×360.

| Picture | Focus 50: mean / p95 / max | Focus 100: mean / p95 / max |
| --- | --- | --- |
| Opposing halves (orange, blue) | 0.02 / 0.06 / 0.09 | 0.03 / 0.08 / 0.17 |
| Magenta and green objects on grey | 0.01 / 0.04 / 0.08 | 0.02 / 0.07 / 0.16 |
| Hue and lightness gradients | 0.02 / 0.03 / 0.04 | 0.02 / 0.04 / 0.08 |
| Seeded random blocks, 16 px | 0.16 / 0.26 / 0.33 | 0.34 / 0.62 / 0.89 |
| Seeded random blocks, 48 px | 0.03 / 0.11 / 0.21 | 0.04 / 0.12 / 0.41 |
| Dark scene | 0.01 / 0.03 / 0.07 | 0.01 / 0.03 / 0.07 |
| Dark random blocks, 16 px | 0.02 / 0.04 / 0.06 | 0.04 / 0.07 / 0.10 |

Target: p95 under one step. Met everywhere; the unit test
`the_two_part_evaluation_agrees_with_every_cell_at_full_resolution` asserts it.

### Accuracy and work by edge depth, focus 100

Worst p95 over the seven pictures (steps), and the colours each LED weighs per frame
(near groups plus blocks, the per-LED work). The full table, with one size changed at a
time, is in [`results.md`](results.md).

| Edge depth | Shipped: colours / p95 | 400 blocks | Single cells only | Groups up to 3 cells |
| ---: | --- | --- | --- | --- |
| 1 % | 230 / 0.54 | 434 / 0.55 | same as shipped | same as shipped |
| 10 % | 234 / 0.62 | 527 / 0.38 | same | same |
| 20 % | 234 / 0.62 | 527 / 0.50 | same | same |
| 30 % | 375 / 0.49 | 668 / 0.43 | same | same |
| 40 % | 229 / 0.54 | 229 / 0.54 | 566 / 0.39 | same |
| 60 % | 355 / 0.37 | 355 / 0.37 | 1059 / 0.34 | 151 / 0.85 |
| 100 % | 686 / 0.47 | 686 / 0.47 | 2347 / 0.17 | 294 / 1.05 |

### Cost on this Mac

`cargo run --release -p prysm-processor --example bench`, 640×360 YUYV, 120 changing
frames, the fastest of five interleaved runs each:

| Edge depth | Focus 0 | Focus 100 | Colour focus costs |
| ---: | ---: | ---: | ---: |
| 10 % | 0.63–0.65 ms | 0.78–0.80 ms | 0.14–0.15 ms |
| 100 % | 0.63–0.65 ms | 0.99–1.03 ms | 0.35–0.39 ms |

Ranges are over four runs. The cost does not depend on the
focus above 0 or on the picture.

On the dev TV (ARMv7, `bench` cross-built with the `webos` profile and run at low
priority while Prysm was syncing):

| Edge depth | Focus 0 | Focus 100 | Difference |
| --- | --- | --- | --- |
| 10 % | 11.4 ms | 19.2 ms | 7.8 ms |
| 100 % | 11.2 ms | 20.8 ms | 9.6 ms |

The pass is about a fifth of the frame's work on the Mac and about two fifths on the
TV; where the TV spends it (cube roots, divisions, or memory) was not profiled.

### Effect

Oklab chroma of the 162 LEDs, unsmoothed, without grain. "Beside the boundary" is the
middle fifth of the top and bottom strips for the halves, and the LEDs next to the two
objects for the grey picture.

| Picture | Focus | All LEDs, mean | Beside the boundary, mean | Beside, lowest | Change from focus 0, mean / max (steps) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Opposing halves | 0 | 0.191 | 0.168 | 0.154 | 0.00 / 0.00 |
| Opposing halves | 50 | 0.202 | 0.187 | 0.153 | 1.34 / 3.59 |
| Opposing halves | 100 | 0.208 | 0.200 | 0.153 | 1.90 / 6.00 |
| Objects on grey | 0 | 0.014 | 0.089 | 0.023 | 0.00 / 0.00 |
| Objects on grey | 50 | 0.017 | 0.122 | 0.019 | 0.24 / 3.63 |
| Objects on grey | 100 | 0.019 | 0.142 | 0.014 | 0.41 / 5.40 |

The pictures' own chroma: orange 0.196, blue 0.222, green 0.220, magenta 0.266. The LED
right on the boundary sees both colours next to it, so its anchor is the mix and it
stays mixed (the unchanged lowest value).

### Flicker

The smoothing study's grain (±3 code values per 8×8 or 32×32 block per frame, mostly
luma), 2 s at 30 fps, frame-to-frame LED change after the first second, mean / p95 in
steps, unsmoothed:

| Focus | Halves, fine: all / beside | Halves, blotchy: all / beside | Objects, fine: all / beside | Objects, blotchy: all / beside |
| ---: | --- | --- | --- | --- |
| 0 | 0.07 / 0.16 · 0.06 / 0.14 | 0.15 / 0.35 · 0.14 / 0.32 | 0.08 / 0.19 · 0.07 / 0.15 | 0.18 / 0.41 · 0.17 / 0.39 |
| 50 | 0.07 / 0.16 · 0.07 / 0.15 | 0.15 / 0.35 · 0.14 / 0.31 | 0.08 / 0.19 · 0.08 / 0.16 | 0.18 / 0.41 · 0.18 / 0.39 |
| 100 | 0.07 / 0.16 · 0.07 / 0.16 | 0.15 / 0.35 · 0.15 / 0.33 | 0.09 / 0.19 · 0.08 / 0.17 | 0.19 / 0.41 · 0.18 / 0.39 |
| 100 against 0, mean | 1.00× · 1.08× | 1.00× · 1.03× | 1.02× · 1.11× | 1.01× · 1.05× |

With the default smoothing of 0.1 s every value is about half, with the same ratios.
Grain changes the hue little, and agreement reads hue and saturation only, so the
weights hardly move. A moving boundary is different (see the finding): focus follows
it about three times as sharply at the LED it passes.

## Method

The accuracy tables come from the processor's own unit tests
(`crates/processor/src/nodes/spatial_sampler/color_focus/tests.rs`). The reference
there evaluates the formula above on every cell for every sample, using the
sampler's own cells, weights and functions. [`selectiverun.rs`](selectiverun.rs) feeds
synthetic 640×360 RGB24 pictures to the real `PrysmProcessor` at 30 fps and reads the
162 LEDs as the WLED output does (top 51, right 31, bottom 49, left 31). Nothing is
downloaded and the grain is seeded, so every run is identical except for the timings.

```sh
WORK=/some/scratch/dir sh run.sh > results.md
```

## Limits

- Synthetic pictures only, made to show the effect; the colourfulness study expects
  a visible difference on about 1 LED in 100 over a film, and 1 in 4 in scenes of
  clashing colours. Nothing here says whether the result looks better on a wall.
- On the TV the pass does not fit a 60 fps capture on one thread (see Results). It was
  not optimised for the TV: that waits for a judgement of whether it is wanted.
- One agreement width (0.08) and one grey discount (1.0), the model's. A narrower
  width would select harder and follow a moving boundary more sharply still.
- The first frame after a geometry change (a new crop, for instance) also builds the
  lists. That costs about as much as the plain weights, which are rebuilt in the same
  frame already; it was not timed separately.
