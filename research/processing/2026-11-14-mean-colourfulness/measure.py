"""How much colourfulness the plain weighted mean loses per LED, on real frames.

Pure Python model of the sampler's kernel on a 64x36 cell grid of a 640x360
picture. For each LED sample compares, in Oklab:
  out     chroma of the weighted mean colour (what Prysm lights today)
  seen    weighted mean of the cells' own chroma (colourfulness in view)
  anchor  chroma of the local lobe only (the colour next to the LED)
  sel     chroma of a selective (bilateral) mean anchored on `anchor`
"""
import math
import sys

W, H, CW, CH = 640, 360, 64, 36
STEP = 2  # every other frame
SIGMA = 0.08  # Oklab a/b agreement width
GREY = 1.0  # extra weight per 0.1 of chroma

LIN = [((v / 255) / 12.92 if v <= 10 else ((v / 255 + 0.055) / 1.055) ** 2.4) for v in range(256)]


def oklab(r, g, b):
    l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b) ** (1 / 3)
    m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b) ** (1 / 3)
    s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b) ** (1 / 3)
    return (
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    )


def leds():
    out = []
    for i in range(82):
        out.append(((i + 0.5) / 82 * W, 0.0))
    for i in range(46):
        out.append((float(W), (i + 0.5) / 46 * H))
    for i in range(82):
        out.append(((i + 0.5) / 82 * W, float(H)))
    for i in range(46):
        out.append((0.0, (i + 0.5) / 46 * H))
    return out


def kernels():
    radius = H * 0.1
    full, local = [], []
    for (sx, sy) in leds():
        f, lo = [], []
        for cy in range(CH):
            for cx in range(CW):
                dx = ((cx + 0.5) * W / CW - sx) / radius
                dy = ((cy + 0.5) * H / CH - sy) / radius
                d2 = dx * dx + dy * dy
                peak = 64.0 / (1 + 16 * d2) ** 2
                f.append(1 / (1 + d2) + peak)
                lo.append(peak)
        tf, tl = sum(f), sum(lo)
        full.append([w / tf for w in f])
        local.append([w / tl for w in lo])
    return full, local


def cells(frame):
    n = (W // CW) * (H // CH)
    out = []
    for cy in range(CH):
        rows = [frame[(y * W) * 3:(y * W + W) * 3] for y in range(cy * 10, cy * 10 + 10)]
        for cx in range(CW):
            r = g = b = 0.0
            for row in rows:
                seg = row[cx * 30:cx * 30 + 30]
                r += sum(LIN[v] for v in seg[0::3])
                g += sum(LIN[v] for v in seg[1::3])
                b += sum(LIN[v] for v in seg[2::3])
            out.append((r / n, g / n, b / n))
    return out


def pct(values, q):
    values = sorted(values)
    return values[min(len(values) - 1, int(q * len(values)))]


def main(path):
    full, local = kernels()
    data = open(path, "rb")
    size = W * H * 3
    per_frame = []
    rows = []  # (L_out, C_out, C_seen, C_anchor, C_sel, dE out->sel)
    index = 0
    while True:
        frame = data.read(size)
        if len(frame) < size:
            break
        index += 1
        if index % STEP or not (LO <= index <= HI) or index in SKIP:
            continue
        rgb = cells(frame)
        lab = [oklab(*c) for c in rgb]
        chroma = [math.hypot(a, b) for (_, a, b) in lab]
        R = [c[0] for c in rgb]
        G = [c[1] for c in rgb]
        B = [c[2] for c in rgb]
        grey = [1 + GREY * c / 0.1 for c in chroma]
        for wf, wl in zip(full, local):
            r = sum(map(float.__mul__, wf, R))
            g = sum(map(float.__mul__, wf, G))
            b = sum(map(float.__mul__, wf, B))
            seen = sum(map(float.__mul__, wf, chroma))
            lo, ao, bo = oklab(r, g, b)
            # Anchor: the local lobe, with grey counted less.
            wa = list(map(float.__mul__, wl, grey))
            ta = sum(wa)
            ar = sum(map(float.__mul__, wa, R)) / ta
            ag = sum(map(float.__mul__, wa, G)) / ta
            ab = sum(map(float.__mul__, wa, B)) / ta
            _, aa, ba = oklab(ar, ag, ab)
            ws = [
                w * gr / (1 + ((a - aa) ** 2 + (b2 - ba) ** 2) / (SIGMA * SIGMA))
                for w, gr, (_, a, b2) in zip(wf, grey, lab)
            ]
            ts = sum(ws)
            sr = sum(map(float.__mul__, ws, R)) / ts
            sg = sum(map(float.__mul__, ws, G)) / ts
            sb = sum(map(float.__mul__, ws, B)) / ts
            # Brightness from the plain mean.
            y_out = 0.2126 * r + 0.7152 * g + 0.0722 * b
            y_sel = 0.2126 * sr + 0.7152 * sg + 0.0722 * sb
            k = y_out / y_sel if y_sel > 1e-9 else 1.0
            ls, as_, bs = oklab(sr * k, sg * k, sb * k)
            rows.append((
                lo,
                math.hypot(ao, bo),
                seen,
                math.hypot(aa, ba),
                math.hypot(as_, bs),
                math.sqrt((ls - lo) ** 2 + (as_ - ao) ** 2 + (bs - bo) ** 2),
            ))
        fr = rows[-256:]
        k = sorted(r[1] / r[2] for r in fr if r[2] > 0.02 and r[0] > 0.2)
        over = sum(r[5] / 0.02 > 2 for r in fr) / 256
        per_frame.append((index, k[len(k) // 2] if k else 1.0, over))
    lit = [row for row in rows if row[0] > 0.2]
    print(f"LED samples: {len(rows)}, lit (L > 0.2): {len(lit)}")
    for name, col in (("out (today)", 1), ("seen", 2), ("anchor", 3), ("selective", 4)):
        v = [row[col] for row in lit]
        print(f"chroma {name:12s} mean {sum(v)/len(v):.4f} median {pct(v, .5):.4f} p90 {pct(v, .9):.4f}")
    kept = [row[1] / row[2] for row in lit if row[2] > 0.02]
    print(f"kept = out/seen, where seen > 0.02 ({len(kept)}): median {pct(kept, .5):.2f} p10 {pct(kept, .1):.2f} share under 0.5: {sum(k < .5 for k in kept)/len(kept):.1%}")
    gain = [row[4] / row[1] for row in lit if row[1] > 0.01]
    print(f"selective/out chroma ratio: median {pct(gain, .5):.2f} p90 {pct(gain, .9):.2f}")
    de = [row[5] / 0.02 for row in lit]
    print(f"selective vs today, in JND (0.02): median {pct(de, .5):.1f} p90 {pct(de, .9):.1f} share over 2 JND: {sum(d > 2 for d in de)/len(de):.1%}")

    worst = sorted(per_frame, key=lambda f: -f[2])[:6]
    print('frames where selective differs most (frame, median kept, share over 2 JND):', [(i, round(k, 2), round(o, 2)) for i, k, o in worst])


LO, HI = int(sys.argv[2]), int(sys.argv[3])
SKIP = set(map(int, sys.argv[4].split(','))) if len(sys.argv) > 4 else set()
main(sys.argv[1])
