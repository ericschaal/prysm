import { useId, type CSSProperties, type ReactNode } from 'react';
import { ledCount, type LedEdges } from './adapters';
import { ledStops } from './live-colors';

export type Edge = keyof LedEdges;
export type Corner = 'bottom-left' | 'top-left' | 'top-right' | 'bottom-right';
/** An edge's LEDs in the wiring: the corner they start from and end at. */
export interface Wire {
  edge: Edge;
  from: Corner;
  to: Corner;
}

type Point = readonly [number, number];

/*
 * Home's TV seen from the front, in viewBox units: the screen, its band `out`
 * beyond each edge and `inset` short of each screen corner, so the four
 * corners are equal empty gaps, and the stand's base below.
 */
const screen = { x: 60, y: 34, width: 200, height: 112 };
const screenRadius = 7;
const out = 9;
const inset = 6;
/** Home's drawing, with room for the counts (the bottom one past the stand). */
const home = { width: 320, height: 200 };
/** Lowers 11px digits so they are centred on their point. */
const digitDrop = 4;

const cx = screen.x + screen.width / 2;
const cy = screen.y + screen.height / 2;
const left = screen.x - out;
const right = screen.x + screen.width + out;
const top = screen.y - out;
const bottom = screen.y + screen.height + out;
/** Where the bands would meet. */
const corners: Record<Corner, Point> = {
  'bottom-left': [left, bottom],
  'top-left': [left, top],
  'top-right': [right, top],
  'bottom-right': [right, bottom],
};
/** Home's counts, beside their bands. */
const countAt: Record<Edge, Point> = {
  top: [cx, top - 12],
  right: [right + 20, cy + 4],
  bottom: [cx, bottom + 35],
  left: [left - 20, cy + 4],
};
const homeView = `0 0 ${home.width} ${home.height}`;
const percent = (part: number, whole: number) => `${(part / whole) * 100}%`;
/** The screen's box on Home's drawing, for HTML laid over it. */
const screenBox = {
  left: percent(screen.x, home.width),
  top: percent(screen.y, home.height),
  width: percent(screen.width, home.width),
  height: percent(screen.height, home.height),
  borderRadius: `${percent(screenRadius, screen.width)} / ${percent(screenRadius, screen.height)}`,
};
const stand = `M${cx - 22} ${screen.y + screen.height + 26}h44`;
/** Home's bands, each drawn clockwise. */
const clockwise: readonly Wire[] = [
  { edge: 'top', from: 'top-left', to: 'top-right' },
  { edge: 'right', from: 'top-right', to: 'bottom-right' },
  { edge: 'bottom', from: 'bottom-right', to: 'bottom-left' },
  { edge: 'left', from: 'bottom-left', to: 'top-left' },
];
/** The way each edge's colours run: left to right, top to bottom. */
const screenWay: Record<Edge, readonly [Corner, Corner]> = {
  top: ['top-left', 'top-right'],
  right: ['top-right', 'bottom-right'],
  bottom: ['bottom-left', 'bottom-right'],
  left: ['top-left', 'bottom-left'],
};
/** The corners in clockwise order, seen from the front. */
const aroundClockwise = clockwise.map(({ from }) => from);

/** The band from one corner to the other, `out + inset` short of each. */
function bandOf([x1, y1]: Point, [x2, y2]: Point) {
  const length = Math.hypot(x2 - x1, y2 - y1);
  const dx = ((x2 - x1) / length) * (out + inset);
  const dy = ((y2 - y1) / length) * (out + inset);
  return { x1: x1 + dx, y1: y1 + dy, x2: x2 - dx, y2: y2 - dy };
}

/** Which way a wire runs on the drawing, from its first corner. */
function wayOf({ from, to }: Wire) {
  const [fromRow, fromSide] = from.split('-');
  if (fromRow !== to.split('-')[0]) return fromRow === 'bottom' ? 'up' : 'down';
  return fromSide === 'left' ? 'right' : 'left';
}

/** Where LED 1 is and which way the strip runs from it, in words. */
function directionOf(first: Wire | undefined) {
  if (!first) return 'No LEDs yet. Enter how many are on each edge.';
  const next = (aroundClockwise.indexOf(first.from) + 1) % 4;
  const turn =
    aroundClockwise[next] === first.to ? 'clockwise' : 'anticlockwise';
  return `First LED is ${first.from.replace('-', ' ')}. The strip runs ${turn}.`;
}

/**
 * The TV seen from the front on its stand, with the LED band around it:
 * each edge's run of light (dashed and dim when it has none) and its count.
 * Home draws it, its counts as numbers, its band lit; it can lay `screen`
 * over the screen (the live preview) and put a `note` under it. The editor
 * (`editing`) builds it in HTML, so the fields keep their size at any width:
 * each edge's count field in the middle of its strip, LED 1's corner badged
 * with one mark along the first lit strip, the total in the screen, marked
 * while it is refused (`invalid`), and the `wiring` in words under it.
 */
export function LedDrawing({
  edges,
  caption,
  screen: onScreen,
  note,
  editing,
}: {
  edges: LedEdges;
  /** The drawing in words, for screen readers. */
  caption: string;
  /** Home only: HTML in the screen's box, clipped to its corners. */
  screen?: ReactNode;
  /** Home only: one line under the drawing. */
  note?: ReactNode;
  editing?: {
    wiring: readonly Wire[];
    invalid: boolean;
    counts: Record<Edge, ReactNode>;
  };
}) {
  const id = useId();
  if (editing) {
    const first = editing.wiring.find(({ edge }) => edges[edge] > 0);
    return (
      <figure className="led-layout">
        <div className="led-tv">
          <div className="led-map-screen">
            <p className="screen-label" data-invalid={editing.invalid}>
              <strong>{ledCount(edges)}</strong> <span>LEDs in total</span>
            </p>
          </div>
          {editing.wiring.map((wire) => (
            <div
              key={wire.edge}
              className={`led-edge led-edge-${wire.edge}${edges[wire.edge] > 0 ? '' : ' empty'}`}
              data-way={wire === first ? wayOf(wire) : undefined}
            >
              <span className="led-strip" />
              {editing.counts[wire.edge]}
              <span className="led-strip" />
            </div>
          ))}
          {first ? (
            <span
              className="led-start"
              data-corner={first.from}
              data-way={wayOf(first)}
              aria-hidden="true"
            >
              1
              <svg viewBox="0 0 14 9" focusable="false">
                <path d="M1 8 7 1l6 7" />
              </svg>
            </span>
          ) : null}
        </div>
        <p className="led-way">
          {first ? (
            <span className="led-badge" aria-hidden="true">
              1
            </span>
          ) : null}
          {directionOf(first)}
        </p>
        <figcaption className="visually-hidden">{caption}</figcaption>
      </figure>
    );
  }
  return (
    <figure className="led-map" data-preview={onScreen ? true : undefined}>
      {/* The drawing's own box, so the screen's box can be laid over it. */}
      <div className="led-map-tv">
        <svg viewBox={homeView} aria-hidden="true" focusable="false">
          <defs>
            {/* The light the band throws on the wall: soft, not an outline. */}
            <filter
              id={`${id}-blur`}
              filterUnits="userSpaceOnUse"
              x="0"
              y="0"
              width={home.width}
              height={home.height}
            >
              <feGaussianBlur stdDeviation="5" />
            </filter>
          </defs>
          <g className="led-map-band">
            {clockwise.map(({ edge, from, to }) => {
              const band = bandOf(corners[from], corners[to]);
              const [first, last] = screenWay[edge];
              const paint = `url(#${id}-${edge})`;
              return (
                <g
                  key={edge}
                  className={`led-map-edge edge-${edge}${edges[edge] > 0 ? '' : ' empty'}`}
                >
                  {/* The edge's colours along its band, in the stream's order. */}
                  <linearGradient
                    id={`${id}-${edge}`}
                    gradientUnits="userSpaceOnUse"
                    {...bandOf(corners[first], corners[last])}
                  >
                    {Array.from({ length: ledStops }, (_, index) => (
                      <stop
                        key={index}
                        offset={(index + 0.5) / ledStops}
                        style={
                          {
                            '--led': `var(--led-${edge[0]}-${index})`,
                          } as CSSProperties
                        }
                      />
                    ))}
                  </linearGradient>
                  <line
                    className="halo"
                    filter={`url(#${id}-blur)`}
                    stroke={paint}
                    {...band}
                  />
                  <line stroke={paint} {...band} />
                </g>
              );
            })}
          </g>
          <path className="led-map-stand" d={stand} />
          <g className="led-map-screen">
            <rect {...screen} rx={screenRadius} />
          </g>
          {clockwise.map(({ edge }) => (
            <text
              key={edge}
              className="led-map-count"
              x={countAt[edge][0]}
              y={countAt[edge][1] + digitDrop}
              textAnchor="middle"
            >
              {edges[edge]}
            </text>
          ))}
        </svg>
        {onScreen ? (
          <div className="led-map-picture" style={screenBox}>
            {onScreen}
          </div>
        ) : null}
      </div>
      {note}
      <figcaption className="visually-hidden">{caption}</figcaption>
    </figure>
  );
}
