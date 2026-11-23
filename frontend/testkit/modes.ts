/** Capture modes for tests: the shapes `POST /api/v1/capture/modes` answers. */
import type { Mode } from '@prysm/contracts/application';

const span = (min: number, max: number, step = 1) => ({ min, max, step });

/** Every even size up to `width` × `height`, at 1 to `fps` fps. */
export const anyMode = (width: number, height: number, fps: number): Mode => ({
  width: span(2, width, 2),
  height: span(2, height, 2),
  fps: span(1, fps),
});

/** One size at one rate. */
export const fixedMode = (
  width: number,
  height: number,
  fps: number,
): Mode => ({
  width: span(width, width),
  height: span(height, height),
  fps: span(fps, fps),
});
