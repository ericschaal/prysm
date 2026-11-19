import type { ReactNode } from 'react';
import type { LedCounts } from '@prysm/contracts/host-desktop';
import {
  ledRuns,
  ledTotal,
  wiring,
  type Corner,
  type Edge,
  type LedRun,
} from './leds';

/** Where each corner is in the diagram, and each strip's line. */
const corners: Record<Corner, [number, number]> = {
  'bottom-left': [8, 142],
  'top-left': [8, 8],
  'top-right': [232, 8],
  'bottom-right': [232, 142],
};
const strips: Record<Edge, { from: [number, number]; to: [number, number] }> = {
  left: { from: [8, 132], to: [8, 18] },
  top: { from: [18, 8], to: [222, 8] },
  right: { from: [232, 18], to: [232, 132] },
  bottom: { from: [222, 142], to: [18, 142] },
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

/**
 * The screen seen from the front with each edge's LEDs, their order and
 * LED 1, and `edges[edge]` (the edge's count field) beside each edge. The
 * total is in the middle, marked while it is refused (`invalid`). The
 * order is also written out, for screen readers.
 */
export function LedLayout({
  leds,
  invalid,
  edges,
}: {
  leds: LedCounts;
  invalid: boolean;
  edges: Record<Edge, ReactNode>;
}) {
  const runs = ledRuns(leds);
  const start = runs.find((run) => run.count > 0);
  return (
    <figure className="led-layout">
      {wiring.map(({ edge }) => (
        <div key={edge} className={`led-edge led-edge-${edge}`}>
          {edges[edge]}
        </div>
      ))}
      <svg viewBox="0 0 240 150" aria-hidden="true" focusable="false">
        <rect className="screen" x="26" y="26" width="188" height="98" rx="6" />
        <text
          className="screen-label"
          data-invalid={invalid}
          x="120"
          y="78"
          textAnchor="middle"
        >
          {ledTotal(leds)} LEDs
        </text>
        <text className="screen-sublabel" x="120" y="98" textAnchor="middle">
          seen from the front
        </text>
        {runs.map((run) => {
          const strip = strips[run.edge];
          const empty = run.count === 0;
          return (
            <g
              key={run.edge}
              className={`strip strip-${run.edge}${empty ? ' empty' : ''}`}
            >
              <line
                x1={strip.from[0]}
                y1={strip.from[1]}
                x2={strip.to[0]}
                y2={strip.to[1]}
              />
              {empty ? null : (
                <polygon
                  className="arrow"
                  points={arrow(strip.from, strip.to)}
                />
              )}
            </g>
          );
        })}
        {start ? (
          <g className="start">
            <circle
              cx={corners[start.from][0]}
              cy={corners[start.from][1]}
              r="8"
            />
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
      <figcaption className="visually-hidden">
        {start
          ? `LED 1 is at the ${start.from} corner. Order: ${runs.map(describeRun).join('; ')}.`
          : 'No LEDs yet.'}
      </figcaption>
    </figure>
  );
}
