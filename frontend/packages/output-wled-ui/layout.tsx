import type { LedCounts } from '@prysm/contracts/host-desktop';
import { ledRuns, ledTotal, type Corner, type Edge, type LedRun } from './leds';

/** Where each corner is in the diagram, and each strip's line. */
const corners: Record<Corner, [number, number]> = {
  'bottom-left': [56, 194],
  'top-left': [56, 36],
  'top-right': [304, 36],
  'bottom-right': [304, 194],
};
const strips: Record<
  Edge,
  { from: [number, number]; to: [number, number]; label: [number, number] }
> = {
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
        <rect
          className="screen"
          x="78"
          y="56"
          width="204"
          height="118"
          rx="6"
        />
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
            <circle
              cx={corners[start.from][0]}
              cy={corners[start.from][1]}
              r="9"
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
      <figcaption>
        {start
          ? `LED 1 is at the ${start.from} corner. Order: ${runs.map(describeRun).join('; ')}.`
          : 'No LEDs yet.'}
      </figcaption>
    </figure>
  );
}
