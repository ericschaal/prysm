import { useEffect, useLayoutEffect, useState, type RefObject } from 'react';
import {
  reconnectDelayMs,
  type ColorsClient,
  type ColorsEvent,
} from '@prysm/application-client';

type Rgb = readonly number[];
type Edge = 'top' | 'right' | 'bottom' | 'left';

type Colors = Extract<ColorsEvent, { type: 'colors' }>;

const edges: readonly Edge[] = ['top', 'right', 'bottom', 'left'];

/**
 * Darker than this, a colour is lifted to it (as grey added): a black scene
 * still shows its LED drawing, and no colour is made up.
 */
const floor = 40;

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

/** A field's lightness: black LEDs rest at `moon`, bright ones reach `full`. */
const lightness = { moon: 0.36, full: 0.66 } as const;
/**
 * From this LED lightness a field is at full strength: mid grey and most
 * full colours reach it (pure blue, the darkest, glows a little lower).
 */
const bright = 0.6;
/** How fast a field sinks as the LEDs dim: more than their lightness does. */
const sink = 1.5;
/** A field's most chroma: rich, never garish. */
const richest = 0.22;
/** Below `none` LEDs have no hue to trust; from `clear` it shows fully. */
const hueFrom = { none: 0.06, clear: 0.16 } as const;

/**
 * One field from the LEDs' colour (OKLab): their hue and saturation, as
 * strong as they are bright. Dimmed LEDs keep their colour and glow lower;
 * grey ones give white light; black ones a faint colourless moonlight. No
 * colour is made up.
 */
function field([L = 0, a = 0, b = 0]: readonly number[]) {
  const light =
    lightness.moon +
    (lightness.full - lightness.moon) * Math.min(1, L / bright) ** sink;
  const shown = Math.min(
    1,
    Math.max(0, (L - hueFrom.none) / (hueFrom.clear - hueFrom.none)),
  );
  const chroma = Math.hypot(a, b);
  // Chroma falls with lightness as LEDs dim: carried to the field's own.
  const scale = chroma
    ? (Math.min(richest, (chroma / L) * light) * shown) / chroma
    : 0;
  return css(srgb([light, a * scale, b * scale]));
}

/**
 * The properties for one `colors` event: each edge's average for the LED
 * drawing (an edge without LEDs takes the average of all), and the card's
 * four fields from the LEDs nearest them: top-left from the top's first
 * third and the left's upper half, top-right likewise, the top-centre field
 * from the top's middle, the low middle field from the bottom and the
 * sides' lower halves (a field without LEDs takes all of them).
 */
export function ledValues(event: Colors) {
  const all = average(edges.flatMap((edge) => event[edge])) ?? [0, 0, 0];
  const values = fieldValues(event);
  for (const edge of edges)
    values[`--led-${edge[0]}`] = css(lift(average(event[edge]) ?? all));
  return values;
}

/** The card's four fields for the LEDs' colours in `event`. */
function fieldValues(event: Colors) {
  const values: Record<string, string> = {};
  const { top, right, bottom, left } = event;
  const whole = mixLab([[edges.flatMap((edge) => event[edge]), 1]]) ?? [0];
  const fields: Record<string, [readonly Rgb[], number][]> = {
    '--live-a': [
      [stretch(top, 0, 3), 1],
      [stretch(left, 0, 2), 1],
    ],
    '--live-b': [
      [stretch(top, 2, 3), 1],
      [stretch(right, 0, 2), 1],
    ],
    '--live-c': [
      [bottom, 2],
      [stretch(left, 1, 2), 1],
      [stretch(right, 1, 2), 1],
    ],
    '--live-d': [[stretch(top, 1, 3), 1]],
  };
  for (const [name, parts] of Object.entries(fields))
    values[name] = field(mixLab(parts) ?? whole);
  return values;
}

/** The LEDs showing nothing. */
const dark: Colors = {
  type: 'colors',
  top: [],
  right: [],
  bottom: [],
  left: [],
};

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

/**
 * While `active`, stream the LEDs' colours onto `target` as CSS custom
 * properties (see `ledProperties`) and say what the glow shows in its
 * `data-live`: `colors`, or `idle` while the lights show none. Until the
 * first colours, on `idle` and while the stream is lost (`data-live`
 * absent) the card's fields are those of black LEDs: the moonlight, never
 * colours the LEDs do not have. Written through the CSSOM, never through
 * React state: no render per event. A failed or ended stream is retried
 * quietly after {@link reconnectDelayMs}.
 */
export function useLiveColors(
  client: ColorsClient | undefined,
  active: boolean,
  target: RefObject<HTMLElement | null>,
) {
  // Before the first paint, so the designed colours never flash.
  useLayoutEffect(() => {
    const element = target.current;
    if (!client || !active || !element) return;
    const show = (values: Record<string, string>) => {
      for (const [name, value] of Object.entries(values))
        element.style.setProperty(name, value);
    };
    show(fieldValues(dark));
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
            // No picture is black LEDs: the card rests in its moonlight.
            show(event.type === 'idle' ? fieldValues(dark) : ledValues(event));
            element.setAttribute('data-live', event.type);
          }
        } catch {
          // Busy or gone: try again later.
        }
        if (signal.aborted) break;
        element.removeAttribute('data-live');
        show(fieldValues(dark));
        await new Promise((resolve) => {
          const timer = setTimeout(resolve, reconnectDelayMs(attempt));
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
