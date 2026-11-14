# How much colour the plain average loses

Date: 2026-11-14. Status: offline model of the spatial sampler on real frames; nothing
was run on the TV or the LEDs. Platform: macOS, Python 3 (standard library only).
Sampler: [`spatial_sampler.rs`](../../../crates/processor/src/nodes/spatial_sampler.rs)
at `57cc93a`, sample density 128, spatial reach 10 %.

## Question

Each LED shows a weighted average of the whole picture. Averaging two different
colours gives a duller one (orange and blue give grey-beige). How much colourfulness
does that cost on real content, and would a sampler that favours the colour next to
each LED win it back?

## Finding

Over a whole film little is lost, but it depends on the scene. The typical LED keeps
86–88 % of the colourfulness in its view, because the kernel's local peak already makes
each LED look mostly at what is next to it, and a selective sampler changes the colour by
more than two just-noticeable steps on about 1 LED in 100. In the few scenes where
strongly different colours share the frame (Tears of Steel's pink and green holograms)
it changes about 1 LED in 4. It would also redo the weights every frame, estimated at
8–12 ms per frame on the TV. Deferred: not worth building for content like these films;
content full of clashing colours (neon, games, music videos) was not measured.

Story frames only (credits and title cards left out):

| | Tears of Steel (live action, muted) | Big Buck Bunny (animation, colourful) |
| --- | --- | --- |
| Frames | 41 | 40 |
| Colourfulness kept, median LED | 86 % | 88 % |
| Colourfulness kept, lowest tenth of LEDs | 57 % | 60 % |
| LEDs keeping less than half | 6.8 % | 5.6 % |
| Selective sampler's chroma gain, median / top tenth | 2 % / 18 % | 9 % / 32 % |
| LEDs where selective differs by more than 2 steps | 1.6 % | 0.6 % |
| The most affected frame: LEDs differing by more than 2 steps | 25 % (hologram scene) | 8 % |

"Kept" is the Oklab chroma of the averaged colour over the weighted mean of the cells'
own chroma, for LEDs whose view has chroma above 0.02. A step is an Oklab distance of
0.02.

The first run (same day) sampled the whole files without looking at the frames: about
one frame in six was credits or a title card. Leaving those out moved the film-wide
numbers by a point or two; the per-scene spread only showed once frames were listed
one by one. Edge depth barely matters: at 10 %, 30 % and 60 % reach the median kept is
90 %, 88 % and 87 % (Big Buck Bunny, 25 frames, credits included), because the kernel's
broad tail brings in the whole picture at any reach.

## Method

[`measure.py`](measure.py) models the sampler in Python: the picture scaled to 640×360,
averaged in linear light into 64×36 cells, and the sampler's kernel
(`1/(1+d²) + 64/(1+16d²)²`, radius 10 % of the height) for 82 + 46 + 82 + 46 samples.
The selective sampler it simulates takes the local peak as an anchor, weights each cell
also by how close its Oklab hue and saturation are to the anchor (width 0.08), counts
grey cells less, and keeps the plain average's brightness.

```sh
ffmpeg -i film.mov -vf "fps=1/7,scale=640:360:flags=area" -pix_fmt rgb24 -f rawvideo frames.rgb
python3 measure.py frames.rgb FIRST LAST [SKIP,SKIP]   # frame numbers, from 1
```

Story frames: Tears of Steel (one frame per 7 s) 2–84 without 10; Big Buck Bunny (one
per 6 s) 1–81 without 4; every other frame of those.

The films are Blender's open movies; they are not kept in the repository.

## Limits

- A model, not the processor: a coarser grid than the real one (64×36 against about
  82×46), and frames scaled by ffmpeg in gamma space.
- Two films, about forty stills each; no motion, so nothing about flicker. Neither has
  many scenes of clashing colours, where the difference is largest.
- Tears of Steel is 2.40:1; its frames were stretched to 16:9, not cropped.
- One setting of the selective sampler; a narrower agreement width would change more
  LEDs, and would also be less stable.
- Numbers, not eyes: whether the remaining loss matters on a wall was not judged.
