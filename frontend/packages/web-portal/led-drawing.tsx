import { useId, type ReactNode } from 'react';
import { ledCount, type LedEdges } from './adapters';

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
 * The TV seen from the front, in viewBox units: the screen, its band `out`
 * beyond each edge and `inset` short of each screen corner, so the four
 * corners are equal empty gaps, and the stand below.
 */
const screen = { x: 60, y: 34, width: 200, height: 112 };
const out = 9;
const inset = 6;
/** Home's drawing, with room for the counts (the bottom one past the stand). */
const home = { width: 320, height: 200 };
/** The editor's drawing: this much round the band on every side. */
const room = 20;
/** The break at a band's middle and its direction mark (half its span). */
const cut = 24;
const mark = 4;
/** LED 1's badge, centred on its corner point: 5 clear of either band. */
const badge = 8;
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
const editorView = `${left - room} ${top - room} ${right - left + 2 * room} ${bottom - top + 2 * room}`;
const stand = `M${cx} ${screen.y + screen.height + 17}v7M${cx - 22} ${screen.y + screen.height + 26}h44`;
/** Each band drawn clockwise when no wiring is given. */
const clockwise: readonly Wire[] = [
  { edge: 'top', from: 'top-left', to: 'top-right' },
  { edge: 'right', from: 'top-right', to: 'bottom-right' },
  { edge: 'bottom', from: 'bottom-right', to: 'bottom-left' },
  { edge: 'left', from: 'bottom-left', to: 'top-left' },
];

const along = ([x, y]: Point, [dx, dy]: Point, by: number): Point => [
  x + dx * by,
  y + dy * by,
];
const ends = ([[x1, y1], [x2, y2]]: readonly [Point, Point]) => ({
  x1,
  y1,
  x2,
  y2,
});

/**
 * The band from one corner to the other, `out + inset` short of each: whole,
 * in two parts round the break at its middle, and the mark in the break
 * pointing from `from` to `to`.
 */
function bandOf(from: Point, to: Point) {
  const length = Math.hypot(to[0] - from[0], to[1] - from[1]);
  const way: Point = [(to[0] - from[0]) / length, (to[1] - from[1]) / length];
  const across: Point = [-way[1], way[0]];
  const start = along(from, way, out + inset);
  const end = along(to, way, -out - inset);
  const middle = along(from, way, length / 2);
  const back = along(middle, way, -mark / 2);
  const points = [
    along(back, across, mark),
    along(middle, way, mark / 2),
    along(back, across, -mark),
  ];
  return {
    whole: [start, end] as const,
    parts: [
      [start, along(middle, way, -cut / 2)],
      [along(middle, way, cut / 2), end],
    ] as const,
    arrow: `M${points.map((point) => point.join(' ')).join('L')}`,
  };
}

/**
 * The TV seen from the front on its stand, with the LED band around it:
 * each edge's run of light (dashed and dim when it has none) and its count.
 * Home shows the counts as numbers. The editor (`editing`) puts each edge's
 * count field beside its band instead, breaks each lit band at its middle
 * for an arrow along the `wiring`, badges LED 1's corner and writes the
 * total in the screen, marked while it is refused (`invalid`).
 */
export function LedDrawing({
  edges,
  caption,
  editing,
}: {
  edges: LedEdges;
  /** The drawing in words, for screen readers. */
  caption: string;
  editing?: {
    wiring: readonly Wire[];
    invalid: boolean;
    counts: Record<Edge, ReactNode>;
  };
}) {
  const blurId = useId();
  const wires = editing?.wiring ?? clockwise;
  const start = editing && wires.find(({ edge }) => edges[edge] > 0)?.from;
  return (
    <figure className={editing ? 'led-layout' : 'led-map'}>
      {editing
        ? wires.map(({ edge }) => (
            <div key={edge} className={`led-edge led-edge-${edge}`}>
              {editing.counts[edge]}
            </div>
          ))
        : null}
      <svg
        viewBox={editing ? editorView : homeView}
        aria-hidden="true"
        focusable="false"
      >
        <defs>
          {/* The light the band throws on the wall: soft, not an outline. */}
          <filter
            id={blurId}
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
          {wires.map(({ edge, from, to }) => {
            const band = bandOf(corners[from], corners[to]);
            const lit = edges[edge] > 0;
            const arrow = editing && lit;
            const lines: readonly (readonly [Point, Point])[] = arrow
              ? band.parts
              : [band.whole];
            return (
              <g
                key={edge}
                className={`led-map-edge edge-${edge}${lit ? '' : ' empty'}`}
              >
                <line
                  className="halo"
                  filter={`url(#${blurId})`}
                  {...ends(band.whole)}
                />
                {lines.map((part) => (
                  <line key={part[0].join()} {...ends(part)} />
                ))}
                {arrow ? <path className="led-map-way" d={band.arrow} /> : null}
              </g>
            );
          })}
        </g>
        <path className="led-map-stand" d={stand} />
        <g className="led-map-screen">
          <rect {...screen} rx="7" />
          {editing ? (
            <text
              className="screen-label"
              data-invalid={editing.invalid}
              x={cx}
              y={cy}
              textAnchor="middle"
              dominantBaseline="central"
            >
              {ledCount(edges)} LEDs
            </text>
          ) : null}
        </g>
        {editing
          ? null
          : clockwise.map(({ edge }) => (
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
        {start ? (
          <g className="led-map-start">
            <circle cx={corners[start][0]} cy={corners[start][1]} r={badge} />
            <text
              x={corners[start][0]}
              y={corners[start][1] + digitDrop}
              textAnchor="middle"
            >
              1
            </text>
          </g>
        ) : null}
      </svg>
      <figcaption className="visually-hidden">{caption}</figcaption>
    </figure>
  );
}
