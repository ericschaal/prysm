#!/usr/bin/env python3
"""Compare per-LED colours written by ledrun (see run.sh). Standard library only.

usage: analyze.py OUT_DIR
"""
import math
import re
import struct
import sys
from pathlib import Path

LEDS = 162
STRIPS = [("top", 51), ("right", 31), ("bottom", 49), ("left", 31)]
RECORD = LEDS * 3 + LEDS * 12
OUT = Path(sys.argv[1])
SIZES = ["1920x1080", "1280x720", "640x360", "320x180"]


def strip_of(led):
    for name, count in STRIPS:
        if led < count:
            return name, led
        led -= count
    raise ValueError


def load(source, size, proc):
    data = (OUT / f"{source}.{size}.{proc}.bin").read_bytes()
    frames = []
    for offset in range(0, len(data), RECORD):
        rec = data[offset : offset + RECORD]
        srgb = rec[: LEDS * 3]
        lin = struct.unpack(f"<{LEDS * 3}f", rec[LEDS * 3 :])
        frames.append((srgb, lin))
    return frames


SOURCES = sorted({p.name.split(".")[0] for p in OUT.glob("*.bin")})
CACHE = {}


def get(source, size, proc):
    key = (source, size, proc)
    if key not in CACHE:
        CACHE[key] = load(*key)
    return CACHE[key]


def pct(values, q):
    if not values:
        return 0
    values = sorted(values)
    return values[min(len(values) - 1, int(math.ceil(q * len(values))) - 1)]


COLUMN = {"head.raw": 1, "head.smooth": 2, "wt.raw": 3, "wt.smooth": 4, "blocks.raw": 5}


def crops(source, size, proc):
    """Normalized confirmed viewport per frame for one processor."""
    key = ("crop", source, size, proc)
    if key not in CACHE:
        w, h = map(int, size.split("x"))
        rows = []
        for line in (OUT / f"{source}.{size}.viewport.txt").read_text().splitlines():
            m = re.search(r"x: (\d+), y: (\d+), width: (\d+), height: (\d+)", line.split("\t")[COLUMN[proc]])
            x, y, vw, vh = map(int, m.groups())
            rows.append((x / w, y / h, vw / w, vh / h))
        CACHE[key] = rows
    return CACHE[key]


# One 360-line row is 0.28 % of the picture: allow rounding, not a moved boundary.
CROP_TOLERANCE = 0.003


def same_crop(source, a, b):
    return [max(abs(u - v) for u, v in zip(p, q)) <= CROP_TOLERANCE for p, q in zip(crops(source, *a), crops(source, *b))]


def compare(a, b, sources, matched=False):
    """a, b: (size, proc). Pooled statistics over sources, LEDs and frames.
    matched: only frames whose black-bar crops agree within CROP_TOLERANCE."""
    chans = [[], [], []]
    signed = 0
    lin_max = 0.0
    lin_sum = 0.0
    lin_n = 0
    worst = []
    per_source = {}
    led_frames = 0
    over = {2: 0, 5: 0, 10: 0}
    for source in sources:
        fa, fb = get(source, *a), get(source, *b)
        assert len(fa) == len(fb), (source, a, b, len(fa), len(fb))
        s_over5 = 0
        s_max = 0
        s_n = 0
        keep = same_crop(source, a, b) if matched else [True] * len(fa)
        for index, ((sa, la), (sb, lb)) in enumerate(zip(fa, fb)):
            if not keep[index]:
                continue
            for led in range(LEDS):
                d = [abs(sa[led * 3 + c] - sb[led * 3 + c]) for c in range(3)]
                for c in range(3):
                    chans[c].append(d[c])
                    signed += sa[led * 3 + c] - sb[led * 3 + c]
                    ld = abs(la[led * 3 + c] - lb[led * 3 + c])
                    lin_max = max(lin_max, ld)
                    lin_sum += ld
                    lin_n += 1
                m = max(d)
                led_frames += 1
                s_n += 1
                for t in over:
                    if m > t:
                        over[t] += 1
                if m > 5:
                    s_over5 += 1
                s_max = max(s_max, m)
                if m >= 6:
                    worst.append((m, source, index, led, tuple(sa[led * 3 : led * 3 + 3]), tuple(sb[led * 3 : led * 3 + 3])))
        per_source[source] = (s_max, s_over5, s_n)
    worst.sort(reverse=True)
    return {
        "chans": [(max(v), sum(v) / len(v), pct(v, 0.95), pct(v, 0.99)) for v in chans],
        "over": over,
        "n": led_frames,
        "lin": (lin_max, lin_sum / lin_n),
        "bias": signed / lin_n,
        "worst": worst[:8],
        "per_source": per_source,
    }


def table(title, rows, sources, matched=False):
    print(f"\n### {title}\n")
    print("| Comparison | frames | R max/mean/p95/p99 | G max/mean/p95/p99 | B max/mean/p95/p99 | >2 | >5 | >10 | mean signed (first - second) | linear max / mean |")
    print("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    results = {}
    for label, a, b in rows:
        r = compare(a, b, sources, matched)
        results[label] = r
        cells = [f"{m} / {mean:.2f} / {p95} / {p99}" for m, mean, p95, p99 in r["chans"]]
        over = [f"{r['over'][t]} ({100 * r['over'][t] / r['n']:.2f} %)" for t in (2, 5, 10)]
        print(f"| {label} | {r['n'] // LEDS} | " + " | ".join(cells + over) + f" | {r['bias']:+.2f} | {r['lin'][0]:.4f} / {r['lin'][1]:.5f} |")
    return results


def per_source_table(results, labels, sources, title):
    print(f"\nPer source, {title}: max step difference (LED-frames > 5 steps)\n")
    print("| Source | frames | " + " | ".join(labels) + " |")
    print("| --- | --- | " + " | ".join("---" for _ in labels) + " |")
    for s in sources:
        frames = len(get(s, "640x360", "wt.raw"))
        cells = []
        for label in labels:
            m, o5, n = results[label]["per_source"][s]
            cells.append(f"{m} ({o5})")
        print(f"| {s} | {frames} | " + " | ".join(cells) + " |")


def worst(results, label, note=""):
    print(f"\nLargest differences, {label}{note} (step, source, frame, strip LED, first RGB, second RGB):\n")
    for m, source, index, led, x, y in results[label]["worst"]:
        name, i = strip_of(led)
        print(f"- {m}: {source} frame {index} ({index / 24:.2f} s), {name} {i}: {x} vs {y}")


def transition(size, proc):
    """Sharpness of the red|blue edge on the top strip.

    ratio = r / (r + b) in linear light per LED. The broad tail of the sampler
    keeps ratios away from 0 and 1, so widths are measured between 25 and 75 %.
    Returns (mean 25-75 % width in LEDs, mean largest ratio step between
    neighbouring LEDs, mean LED count with 10 % < ratio < 90 %).
    """
    widths, steps, counts = [], [], []
    for index, (_, lin) in enumerate(get("sweep", size, proc)):
        edge = 192 * index / 24 / 1920  # normalized edge position
        if not 0.2 < edge < 0.8:
            continue
        ratio = [lin[l * 3] / (lin[l * 3] + lin[l * 3 + 2]) for l in range(51)]

        def cross(level):
            for i in range(50):
                if ratio[i] >= level > ratio[i + 1]:
                    return i + (ratio[i] - level) / (ratio[i] - ratio[i + 1])
            raise ValueError(level)

        widths.append(cross(0.25) - cross(0.75))
        steps.append(max(ratio[i] - ratio[i + 1] for i in range(50)))
        counts.append(sum(1 for v in ratio if 0.1 < v < 0.9))
    n = len(widths)
    return sum(widths) / n, sum(steps) / n, sum(counts) / n


real = [s for s in SOURCES if s.startswith(("sintel", "goldengate"))]
synthetic = [s for s in SOURCES if s not in real and s != "sweep"]
A = ("1280x720", "head.raw")
B = ("1280x720", "wt.raw")
C = ("640x360", "wt.raw")
D1 = ("1920x1080", "head.raw")
D3 = ("320x180", "wt.raw")
E = ("1280x720", "blocks.raw")  # every pixel, capped 81x45 grid
rows = [
    ("A vs C (1280 full vs 640 box)", A, C),
    ("A vs B (1280 full vs 1280 lattice)", A, B),
    ("B vs C (1280 lattice vs 640 box)", B, C),
    ("D1080 full vs A", D1, A),
    ("D1080 full vs C", D1, C),
    ("D1080 full vs 1080 lattice", D1, ("1920x1080", "wt.raw")),
    ("C vs D320 box", C, D3),
    ("check: 640 HEAD vs 640 WT", ("640x360", "head.raw"), C),
    ("A vs E (grid cap only)", A, E),
    ("E vs B (point lattice only)", E, B),
    ("E vs C (TV-style scaler only)", E, C),
]
smooth_rows = [
    ("A vs C, smoothed", ("1280x720", "head.smooth"), ("640x360", "wt.smooth")),
    ("A vs B, smoothed", ("1280x720", "head.smooth"), ("1280x720", "wt.smooth")),
    ("B vs C, smoothed", ("1280x720", "wt.smooth"), ("640x360", "wt.smooth")),
]
print("# Resolution detail: generated tables\n")
print("Generated by analyze.py (see README.md). Sources:", ", ".join(SOURCES))
r_real = table("Real content (no smoothing), 8-bit sRGB steps, all frames", rows, real)
r_real_m = table("Real content (no smoothing), frames whose black-bar crops agree", rows, real, matched=True)
per_source_table(r_real, [l for l, _, _ in rows[:5]], real, "all frames")
per_source_table(r_real_m, [l for l, _, _ in rows[:5]], real, "crops agree")
for label in ("A vs C (1280 full vs 640 box)", "A vs B (1280 full vs 1280 lattice)", "B vs C (1280 lattice vs 640 box)"):
    worst(r_real, label)
    worst(r_real_m, label, ", crops agree")
r_syn = table("Synthetic content (no smoothing)", rows, synthetic)
per_source_table(r_syn, [l for l, _, _ in rows[:5]], synthetic, "all frames")
for label in ("A vs C (1280 full vs 640 box)", "A vs B (1280 full vs 1280 lattice)"):
    worst(r_syn, label)
table("Real content with 0.1 s smoothing, frames whose crops agree", smooth_rows, real, matched=True)
table("Synthetic content with 0.1 s smoothing", smooth_rows, synthetic)

print("\n### Edge sweep on the top strip (frames with the edge in the middle 60 %)\n")
print("| Case | grid | 25-75 % width (LEDs) | largest step between neighbouring LEDs | LEDs between 10 and 90 % |")
print("| --- | --- | --- | --- | --- |")
for label, size, proc, grid in [
    ("D1080 HEAD", "1920x1080", "head.raw", "128x72"),
    ("1080 WT lattice", "1920x1080", "wt.raw", "81x45"),
    ("A 1280 HEAD", "1280x720", "head.raw", "128x72"),
    ("B 1280 WT lattice", "1280x720", "wt.raw", "81x45"),
    ("C 640 box", "640x360", "wt.raw", "81x45"),
    ("D320 box", "320x180", "wt.raw", "64x36"),
    ("E 1280 every pixel", "1280x720", "blocks.raw", "81x45"),
]:
    width, step, count = transition(size, proc)
    print(f"| {label} | {grid} | {width:.2f} | {step:.3f} | {count:.2f} |")

print(f"\n### Black-bar crop agreement (frames whose normalized crop differs by > {CROP_TOLERANCE:.1%})\n")
for s in SOURCES:
    n = len(get(s, *C))
    cells = []
    for label, a, b in rows[:5]:
        cells.append(f"{label.split(' (')[0]} {n - sum(same_crop(s, a, b))}")
    print(f"- {s} ({n} frames): " + ", ".join(cells))
