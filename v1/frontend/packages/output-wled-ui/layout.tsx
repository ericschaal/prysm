import type { LedCounts } from '@prysm/contracts/host-desktop';

export type Edge = keyof LedCounts;

/** The total LED count WLED accepts (`LedCounts::total` in `output-wled`). */
export const ledTotalLimits = { min: 1, max: 65535 } as const;
/** Per edge, from the generated `LedCounts` schema. */
export const ledEdgeLimits = { min: 0, max: 65535, step: 1 } as const;

type Corner = 'bottom-left' | 'top-left' | 'top-right' | 'bottom-right';

/**
 * The physical LED order, as `prysm-renderer-wled` `render_edges` writes it:
 * LED 1 at the bottom-left corner seen from the front, then clockwise — left
 * edge upward, top rightward, right downward, bottom leftward. An edge with no
 * LEDs is skipped.
 */
export const wiring = [
  { edge: 'left', from: 'bottom-left', to: 'top-left', direction: 'upward' },
  { edge: 'top', from: 'top-left', to: 'top-right', direction: 'rightward' },
  { edge: 'right', from: 'top-right', to: 'bottom-right', direction: 'downward' },
  { edge: 'bottom', from: 'bottom-right', to: 'bottom-left', direction: 'leftward' },
] as const satisfies readonly {
  edge: Edge;
  from: Corner;
  to: Corner;
  direction: string;
}[];

export interface LedRun {
  edge: Edge;
  count: number;
  /** 1-based number of the run's first and last LED; absent when empty. */
  first?: number;
  last?: number;
  from: Corner;
  direction: string;
}

export function ledRuns(leds: LedCounts): LedRun[] {
  let next = 1;
  return wiring.map(({ edge, from, direction }) => {
    const count = leds[edge] ?? 0;
    const run: LedRun = { edge, count, from, direction };
    if (count > 0) {
      run.first = next;
      run.last = next + count - 1;
      next += count;
    }
    return run;
  });
}

export function ledTotal(leds: LedCounts) {
  return wiring.reduce((sum, { edge }) => sum + (leds[edge] ?? 0), 0);
}

/** Where each corner is in the diagram, and each strip's line. */
const corners: Record<Corner, [number, number]> = {
  'bottom-left': [56, 194],
  'top-left': [56, 36],
  'top-right': [304, 36],
  'bottom-right': [304, 194],
};
const strips: Record<Edge, { from: [number, number]; to: [number, number]; label: [number, number] }> = {
  left: { from: [56, 186], to: [56, 44], label: [26, 119] },
  top: { from: [64, 36], to: [296, 36], label: [180, 20] },
  right: { from: [304, 44], to: [304, 186], label: [334, 119] },
  bottom: { from: [296, 194], to: [64, 194], label: [180, 222] },
};

/** An arrowhead at the middle of a strip, pointing along the wiring. */
function arrow(from: [number, number], to: [number, number]) {
  const [x, y] = [(from[0] + to[0]) / 2, (from[1] + to[1]) / 2];
  const length = Math.hypot(to[0] - from[0], to[1] - from[1]);
  const [dx, dy] = [(to[0] - from[0]) / length, (to[1] - from[1]) / length];
  const tip = [x + dx * 7, y + dy * 7];
  const left = [x - dx * 5 - dy * 6, y - dy * 5 + dx * 6];
  const right = [x - dx * 5 + dy * 6, y - dy * 5 - dx * 6];
  return [tip, left, right].map((point) => point.join(',')).join(' ');
}

const describeRun = (run: LedRun) =>
  run.first === undefined
    ? `${run.edge}: none`
    : `${run.edge} ${run.direction}: LEDs ${run.first}–${run.last}`;

/** The screen seen from the front with each edge's LEDs, order and LED 1. */
export function LedLayout({ leds }: { leds: LedCounts }) {
  const runs = ledRuns(leds);
  const start = runs.find((run) => run.count > 0);
  const total = ledTotal(leds);
  return (
    <figure className="led-layout">
      <svg viewBox="0 0 360 236" aria-hidden="true" focusable="false">
        <rect className="screen" x="78" y="56" width="204" height="118" rx="6" />
        <text className="screen-label" x="180" y="112" textAnchor="middle">
          {total} LEDs
        </text>
        <text className="screen-sublabel" x="180" y="132" textAnchor="middle">
          seen from the front
        </text>
        {runs.map((run) => {
          const strip = strips[run.edge];
          const empty = run.count === 0;
          return (
            <g key={run.edge} className={empty ? 'strip empty' : 'strip'}>
              <line
                x1={strip.from[0]}
                y1={strip.from[1]}
                x2={strip.to[0]}
                y2={strip.to[1]}
              />
              {empty ? null : (
                <polygon className="arrow" points={arrow(strip.from, strip.to)} />
              )}
              <text
                className="count"
                x={strip.label[0]}
                y={strip.label[1]}
                textAnchor="middle"
              >
                {run.count}
              </text>
            </g>
          );
        })}
        {start ? (
          <g className="start">
            <circle cx={corners[start.from][0]} cy={corners[start.from][1]} r="9" />
            <text
              x={corners[start.from][0]}
              y={corners[start.from][1] + 4}
              textAnchor="middle"
            >
              1
            </text>
          </g>
        ) : null}
      </svg>
      <figcaption>
        {start
          ? `LED 1 is at the ${start.from} corner. Order: ${runs.map(describeRun).join('; ')}.`
          : 'No LEDs yet.'}
      </figcaption>
    </figure>
  );
}
