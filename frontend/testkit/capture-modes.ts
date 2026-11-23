import {
  ApplicationError,
  type CaptureModesClient,
  type CaptureSettings,
  type Failure,
  type Modes,
} from '@prysm/application-client';
import { anyMode } from './modes';

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
