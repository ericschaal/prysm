import type { LedCounts } from '@prysm/contracts/host-desktop';
import {
  clockwiseWires,
  type Corner,
  type Edge,
  type LedEdges,
} from '@prysm/web-portal';

/**
 * The total LED count Prysm accepts: at most `MAX_LEDS` in `output-wled`,
 * the most a WLED device drives.
 */
export const ledTotalLimits = { min: 1, max: 16384 } as const;
/** Per edge, from the generated `LedCounts` schema: one edge may hold all. */
export const ledEdgeLimits = { min: 0, max: 16384, step: 1 } as const;

/**
 * The physical LED order, as output-wled's `render_edges` writes it:
 * LED 1 at the bottom-left corner seen from the front, then clockwise — left
 * edge upward, top rightward, right downward, bottom leftward. An edge with no
 * LEDs is skipped.
 */
export const wiring = [
  clockwiseWires.left,
  clockwiseWires.top,
  clockwiseWires.right,
  clockwiseWires.bottom,
] as const;

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

/** Every edge's count, an omitted edge as none. */
export const ledEdges = (leds: LedCounts): LedEdges => ({
  top: leds.top ?? 0,
  right: leds.right ?? 0,
  bottom: leds.bottom ?? 0,
  left: leds.left ?? 0,
});

export function ledTotal(leds: LedCounts) {
  return wiring.reduce((sum, { edge }) => sum + (leds[edge] ?? 0), 0);
}
