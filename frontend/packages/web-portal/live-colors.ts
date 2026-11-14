import { useEffect, useState, type RefObject } from 'react';
import type { ColorsClient, ColorsEvent } from '@prysm/application-client';

type Rgb = readonly number[];
type Edge = 'top' | 'right' | 'bottom' | 'left';

const edges: readonly Edge[] = ['top', 'right', 'bottom', 'left'];

/**
 * Darker than this, a colour is lifted to it (as grey added): a black scene
 * still shows its LED drawing, and no colour is made up.
 */
const floor = 40;

/**
 * The designed screen sync light (`--aurora-1`…`-4` in tokens.css): what a
 * grey or black scene shows, and what every colour leans toward a little.
 */
export const designedFields = {
  a: [224, 71, 158],
  b: [34, 195, 238],
  c: [107, 77, 255],
  d: [255, 138, 76],
} as const;

/**
 * The CSS custom properties written: the edge averages `--led-t`… (the LED
 * drawing in Your setup) and the card's four light fields `--live-a`…`-d`.
 */
export const ledProperties = [
  ...edges.map((edge) => `--led-${edge[0]}`),
  '--live-a',
  '--live-b',
  '--live-c',
  '--live-d',
];

/** The average of `colors`, or `null` for none. */
function average(colors: readonly Rgb[]): Rgb | null {
  if (!colors.length) return null;
  let [r, g, b] = [0, 0, 0];
  for (const [red = 0, green = 0, blue = 0] of colors) {
    r += red;
    g += green;
    b += blue;
  }
  return [r, g, b].map((value) => value / colors.length);
}

/** Part `index` of `parts` equal stretches of `colors` (all of them when short). */
function stretch(colors: readonly Rgb[], index: number, parts: number) {
  if (colors.length < parts) return colors;
  const from = Math.floor((index * colors.length) / parts);
  const to = Math.floor(((index + 1) * colors.length) / parts);
  return colors.slice(from, Math.max(to, from + 1));
}

function lift(color: Rgb): Rgb {
  const brightest = Math.max(...color);
  if (brightest >= floor) return color;
  return color.map((value) => value + floor - brightest);
}

const css = (color: Rgb) =>
  `rgb(${color.map((value) => Math.round(value)).join(' ')})`;

// sRGB bytes <-> OKLab (Björn Ottosson's matrices).
const toLinear = (c: number) => {
  const v = c / 255;
  return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
};
const fromLinear = (v: number) => {
  const c = Math.min(1, Math.max(0, v));
  return 255 * (c <= 0.0031308 ? c * 12.92 : 1.055 * c ** (1 / 2.4) - 0.055);
};
function oklab([r = 0, g = 0, b = 0]: Rgb): [number, number, number] {
  const [lr, lg, lb] = [toLinear(r), toLinear(g), toLinear(b)];
  const l = Math.cbrt(
    0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb,
  );
  const m = Math.cbrt(
    0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb,
  );
  const s = Math.cbrt(
    0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb,
  );
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}
function srgb([L = 0, a = 0, b = 0]: readonly number[]): Rgb {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ].map(fromLinear);
}
/** The OKLab average of weighted colours. */
function mixLab(parts: readonly [readonly Rgb[], number][]) {
  const sum = [0, 0, 0];
  let total = 0;
  for (const [colors, weight] of parts)
    for (const color of colors) {
      oklab(color).forEach((value, index) => {
        sum[index] = (sum[index] ?? 0) + value * weight;
      });
      total += weight;
    }
  return total ? sum.map((value) => value / total) : null;
}

/** Lightness and chroma the fields keep: rich, never muddy or blown out. */
const range = { lightness: [0.5, 0.66], chroma: [0.12, 0.22] } as const;
/** From this chroma a scene counts as vivid; below this lightness, as dark. */
const vividChroma = 0.12;
const dark = 0.4;
/** Even vivid content keeps this much of the designed light. */
const designedShare = 0.15;

/**
 * One field: the LEDs' colour (OKLab) held in the lightness and chroma
 * window with its hue kept, blended with the designed field. Grey or black
 * content gives the designed field; vivid content leads.
 */
function field(lab: readonly number[] | null, designed: Rgb) {
  if (!lab) return css(designed);
  const [L = 0, a = 0, b = 0] = lab;
  const chroma = Math.hypot(a, b);
  const hue = Math.atan2(b, a);
  const clamp = (value: number, [min, max]: readonly [number, number]) =>
    Math.min(max, Math.max(min, value));
  const held = [
    clamp(L, range.lightness),
    clamp(chroma, range.chroma) * Math.cos(hue),
    clamp(chroma, range.chroma) * Math.sin(hue),
  ];
  const vivid = Math.min(
    Math.min(1, chroma / vividChroma),
    Math.min(1, L / dark),
  );
  const live = (1 - designedShare) * vivid;
  const target = oklab(designed);
  return css(
    srgb(
      held.map(
        (value, index) =>
          (target[index] ?? 0) + (value - (target[index] ?? 0)) * live,
      ),
    ),
  );
}

/**
 * The properties for one `colors` event: each edge's average for the LED
 * drawing (an edge without LEDs takes the average of all), and the card's
 * four fields from the LEDs nearest them: top-left from the top's first
 * third and the left's upper half, top-right likewise, the top-centre field
 * from the top's middle, the low middle field from the bottom and the
 * sides' lower halves.
 */
export function ledValues(event: Extract<ColorsEvent, { type: 'colors' }>) {
  const all = average(edges.flatMap((edge) => event[edge])) ?? [0, 0, 0];
  const values: Record<string, string> = {};
  for (const edge of edges)
    values[`--led-${edge[0]}`] = css(lift(average(event[edge]) ?? all));
  const { top, right, bottom, left } = event;
  values['--live-a'] = field(
    mixLab([
      [stretch(top, 0, 3), 1],
      [stretch(left, 0, 2), 1],
    ]),
    designedFields.a,
  );
  values['--live-b'] = field(
    mixLab([
      [stretch(top, 2, 3), 1],
      [stretch(right, 0, 2), 1],
    ]),
    designedFields.b,
  );
  values['--live-c'] = field(
    mixLab([
      [bottom, 2],
      [stretch(left, 1, 2), 1],
      [stretch(right, 1, 2), 1],
    ]),
    designedFields.c,
  );
  values['--live-d'] = field(
    mixLab([[stretch(top, 1, 3), 1]]),
    designedFields.d,
  );
  return values;
}

/** Whether the page is shown (not a background tab). */
export function usePageVisible() {
  const [visible, setVisible] = useState(
    () => document.visibilityState !== 'hidden',
  );
  useEffect(() => {
    const changed = () => setVisible(document.visibilityState !== 'hidden');
    document.addEventListener('visibilitychange', changed);
    return () => document.removeEventListener('visibilitychange', changed);
  }, []);
  return visible;
}

/** Wait before reconnecting: 1 s, 2 s, 4 s … up to 30 s. */
export const colorsRetryMs = (attempt: number) =>
  Math.min(30_000, 1000 * 2 ** attempt);

/**
 * While `active`, stream the LEDs' colours onto `target` as CSS custom
 * properties (see `ledProperties`) and say what the glow shows in its
 * `data-live`: `colors`, or `idle` while the lights show none. Written
 * through the CSSOM, never through React state: no render per event. A
 * failed or ended stream is retried quietly with backoff; meanwhile
 * `data-live` is absent and the glow is the designed one.
 */
export function useLiveColors(
  client: ColorsClient | undefined,
  active: boolean,
  target: RefObject<HTMLElement | null>,
) {
  useEffect(() => {
    const element = target.current;
    if (!client || !active || !element) return;
    const abort = new AbortController();
    const { signal } = abort;
    void (async () => {
      let attempt = 0;
      while (!signal.aborted) {
        try {
          const stream = await client.colors(signal);
          signal.addEventListener('abort', () => stream.close(), {
            once: true,
          });
          for await (const event of stream.events) {
            if (signal.aborted) break;
            attempt = 0;
            if (event.type === 'idle')
              element.setAttribute('data-live', 'idle');
            else {
              for (const [name, value] of Object.entries(ledValues(event)))
                element.style.setProperty(name, value);
              element.setAttribute('data-live', 'colors');
            }
          }
        } catch {
          // Busy or gone: the designed glow stays; try again later.
        }
        if (signal.aborted) break;
        element.removeAttribute('data-live');
        await new Promise((resolve) => {
          const timer = setTimeout(resolve, colorsRetryMs(attempt));
          signal.addEventListener(
            'abort',
            () => {
              clearTimeout(timer);
              resolve(undefined);
            },
            { once: true },
          );
        });
        attempt++;
      }
    })();
    return () => {
      abort.abort();
      element.removeAttribute('data-live');
    };
  }, [client, active, target]);
}
