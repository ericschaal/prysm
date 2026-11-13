import type { LedCounts } from '@prysm/contracts/host-desktop';

export type Edge = keyof LedCounts;

/** The total LED count WLED accepts (`LedCounts::total` in `output-wled`). */
export const ledTotalLimits = { min: 1, max: 65535 } as const;
/** Per edge, from the generated `LedCounts` schema. */
export const ledEdgeLimits = { min: 0, max: 65535, step: 1 } as const;

export type Corner = 'bottom-left' | 'top-left' | 'top-right' | 'bottom-right';

/**
 * The physical LED order, as `prysm-renderer-wled` `render_edges` writes it:
 * LED 1 at the bottom-left corner seen from the front, then clockwise — left
 * edge upward, top rightward, right downward, bottom leftward. An edge with no
 * LEDs is skipped.
 */
export const wiring = [
  { edge: 'left', from: 'bottom-left', to: 'top-left', direction: 'upward' },
  { edge: 'top', from: 'top-left', to: 'top-right', direction: 'rightward' },
  {
    edge: 'right',
    from: 'top-right',
    to: 'bottom-right',
    direction: 'downward',
  },
  {
    edge: 'bottom',
    from: 'bottom-right',
    to: 'bottom-left',
    direction: 'leftward',
  },
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
