import {
  ApplicationError,
  type CaptureModesClient,
  type CaptureSettings,
  type Failure,
  type Mode,
  type Modes,
} from '@prysm/application-client';

const span = (min: number, max: number, step = 1) => ({ min, max, step });

/** A mode of every size and rate in these bounds. */
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

/**
 * What Prysm's own adapters answer, roughly: any size and rate up to
 * 1920 × 1080 at 60 fps (a file up to 3840 × 2160), for every device,
 * file, TV and encoding.
 */
export function everything(capture: CaptureSettings): Modes {
  return {
    status: 'known',
    modes: [
      capture.kind === 'file'
        ? anyMode(3840, 2160, 120)
        : anyMode(1920, 1080, 60),
    ],
  };
}

/**
 * In-memory capture modes: answers each section with `answer` (by default
 * `everything`), records each question and can refuse the next one.
 */
export function fakeCaptureModesClient(
  answer: (capture: CaptureSettings) => Modes = everything,
) {
  const calls: CaptureSettings[] = [];
  let refusal: Failure | null = null;
  const client: CaptureModesClient = {
    modes: (capture) => {
      calls.push(capture);
      const failure = refusal;
      refusal = null;
      return failure
        ? Promise.reject(new ApplicationError(failure))
        : Promise.resolve(answer(capture));
    },
  };
  return {
    client,
    calls,
    refuseNext: (failure: Failure) => {
      refusal = failure;
    },
  };
}
