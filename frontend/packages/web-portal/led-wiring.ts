import type { LedEdges } from './adapters';

export type Edge = keyof LedEdges;
export type Corner = 'bottom-left' | 'top-left' | 'top-right' | 'bottom-right';
/** An edge's LEDs in the wiring: the corner they start from and end at. */
export interface Wire {
  edge: Edge;
  from: Corner;
  to: Corner;
}

/** The four clockwise runs, seen from the front. Each adapter chooses its strip order. */
export const clockwiseWires = {
  top: {
    edge: 'top',
    from: 'top-left',
    to: 'top-right',
    direction: 'rightward',
  },
  right: {
    edge: 'right',
    from: 'top-right',
    to: 'bottom-right',
    direction: 'downward',
  },
  bottom: {
    edge: 'bottom',
    from: 'bottom-right',
    to: 'bottom-left',
    direction: 'leftward',
  },
  left: {
    edge: 'left',
    from: 'bottom-left',
    to: 'top-left',
    direction: 'upward',
  },
} as const satisfies Record<Edge, Wire & { direction: string }>;
