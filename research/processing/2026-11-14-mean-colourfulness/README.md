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

Little is lost, and a selective sampler would rarely be visible. The typical LED keeps
85–90 % of the colourfulness in its view, because the kernel's local peak already makes
each LED look mostly at what is next to it. A selective sampler changes the colour by
more than two just-noticeable steps on about 1 LED in 100. It would also redo the
weights every frame, estimated at 8–12 ms per frame on the TV. It is not worth building;
a plain saturation control gives a more colourful wall for almost nothing.

| | Tears of Steel (live action, muted) | Big Buck Bunny (animation, colourful) |
| --- | --- | --- |
| Frames (every other one of one per 7 s / 6 s) | 52 | 49 |
| Colourfulness kept, median LED | 85 % | 90 % |
| Colourfulness kept, lowest tenth of LEDs | 54 % | 61 % |
| LEDs keeping less than half | 8.2 % | 5.5 % |
| Selective sampler's chroma gain, median / top tenth | 3 % / 24 % | 8 % / 31 % |
| LEDs where selective differs by more than 2 steps | 1.4 % | 0.8 % |

"Kept" is the Oklab chroma of the averaged colour over the weighted mean of the cells'
own chroma, for LEDs whose view has chroma above 0.02. A step is an Oklab distance of
0.02.

## Method

[`measure.py`](measure.py) models the sampler in Python: the picture scaled to 640×360,
averaged in linear light into 64×36 cells, and the sampler's kernel
(`1/(1+d²) + 64/(1+16d²)²`, radius 10 % of the height) for 82 + 46 + 82 + 46 samples.
The selective sampler it simulates takes the local peak as an anchor, weights each cell
also by how close its Oklab hue and saturation are to the anchor (width 0.08), counts
grey cells less, and keeps the plain average's brightness.

```sh
ffmpeg -i film.mov -vf "fps=1/7,scale=640:360:flags=area" -pix_fmt rgb24 -f rawvideo frames.rgb
python3 measure.py frames.rgb
```

The films are Blender's open movies; they are not kept in the repository.

## Limits

- A model, not the processor: a coarser grid than the real one (64×36 against about
  82×46), and frames scaled by ffmpeg in gamma space.
- Two films, about fifty stills each; no motion, so nothing about flicker.
- One setting of the selective sampler; a narrower agreement width would change more
  LEDs, and would also be less stable.
- Numbers, not eyes: whether the remaining loss matters on a wall was not judged.
