import type { LedCounts } from '@prysm/contracts/host-desktop';
import { clockwiseWires, type LedEdges } from '@prysm/web-portal';

/**
 * The total LED count Prysm accepts: at most `MAX_LEDS` in `output-wled`,
 * the most a WLED device drives.
 */
export const ledTotalLimits = { min: 1, max: 16384 } as const;
/** Per edge, from the generated `LedCounts` schema: one edge may hold all. */
export const ledEdgeLimits = { min: 0, max: 16384, step: 1 } as const;
export const ledTotalProblem = `Use ${ledTotalLimits.min} to ${ledTotalLimits.max} LEDs in total.`;
export const ditherDefault = true;

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

/** Where LED 1 is and the order of every edge, for screen readers. */
export function orderCaption(leds: LedEdges) {
  const start = wiring.find(({ edge }) => leds[edge] > 0);
  if (!start) return 'No LEDs yet.';
  let next = 1;
  const runs = wiring.map(({ edge, direction }) => {
    const count = leds[edge];
    if (!(count > 0)) return `${edge}: none`;
    const range = `${next}–${next + count - 1}`;
    next += count;
    return `${edge} ${direction}: LEDs ${range}`;
  });
  return `LED 1 is at the ${start.from} corner. Order: ${runs.join('; ')}.`;
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
