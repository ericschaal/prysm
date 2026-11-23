import { createContext, use, useEffect, useState } from 'react';
import type {
  CaptureModesClient,
  CaptureSettings,
  Mode,
} from '@prysm/application-client';
import type { Picture } from './adapters';
import {
  picturePresets,
  presetOrder,
  type PicturePreset,
} from './picture-presets';

/** The portal's lookup of what a capture delivers; set by every host. */
export const CaptureModesContext = createContext<CaptureModesClient | null>(
  null,
);

/**
 * What a capture delivers, as far as the portal knows: still asking, its
 * hardware out of reach, the question itself unanswered, or its modes
 * (none: nothing with these settings). Without an answer, `again` asks
 * once more: nothing announces one, as the state stream carries neither a
 * device being plugged in nor the offer a capture-only TV sends when it
 * tries to connect.
 */
export type ModesState =
  | { state: 'loading' }
  | { state: 'unknown'; again: () => void }
  | { state: 'failed'; again: () => void }
  | { state: 'known'; modes: readonly Mode[] };

type Span = Mode['width'];
type Size = Required<Picture>;

const contains = (span: Span, value: number) =>
  value >= span.min &&
  value <= span.max &&
  (value - span.min) % span.step === 0;

/** The smallest value of `span` that is at least `target`, if any. */
function atLeast(span: Span, target: number) {
  if (target <= span.min) return span.min;
  if (target > span.max) return null;
  return span.min + Math.ceil((target - span.min) / span.step) * span.step;
}

/** Whether some mode delivers exactly this size and rate. */
export const delivers = (
  modes: readonly Mode[],
  { width, height, fps }: Size,
) =>
  modes.some(
    (mode) =>
      contains(mode.width, width) &&
      contains(mode.height, height) &&
      contains(mode.fps, fps),
  );

const wide = ({ width, height }: Size) => width * 9 === height * 16;

/**
 * What `modes` deliver for `target`: the target itself where a mode takes
 * it, else the smallest size (16:9 first) at least as large at the same
 * rate; `null` when nothing covers it.
 */
export function resolve(modes: readonly Mode[], target: Size): Size | null {
  if (delivers(modes, target)) return target;
  const covering = modes.flatMap((mode) => {
    const width = atLeast(mode.width, target.width);
    const height = atLeast(mode.height, target.height);
    return width !== null && height !== null && contains(mode.fps, target.fps)
      ? [{ width, height, fps: target.fps }]
      : [];
  });
  const pool = covering.some(wide) ? covering.filter(wide) : covering;
  return pool.reduce<Size | null>(
    (best, size) =>
      best === null || size.width * size.height < best.width * best.height
        ? size
        : best,
    null,
  );
}

/** A preset as `modes` deliver it. */
export interface Offered extends Size {
  preset: PicturePreset;
}

export const sameSize = (a: Size, b: Size) =>
  a.width === b.width && a.height === b.height && a.fps === b.fps;

/**
 * The presets `modes` deliver, in order, each at the size and rate it
 * would set; presets that come out the same show once, under the first.
 */
export function offeredPresets(modes: readonly Mode[]): Offered[] {
  return presetOrder.reduce<Offered[]>((offered, preset) => {
    const size = resolve(modes, picturePresets[preset]);
    return size && !offered.some((other) => sameSize(other, size))
      ? [...offered, { preset, ...size }]
      : offered;
  }, []);
}

/** Whether `modes` leave nothing to choose: no mode, or none for a preset. */
export const nothingOffered = (modes: ModesState) =>
  modes.state === 'known' && offeredPresets(modes.modes).length === 0;

/**
 * Ask what each capture in `queries` delivers: once for each set of
 * queries, and once more each time an unanswered state's `again` is
 * called. A query holds only the fields that select hardware
 * (`{ kind, device }`), so the answer follows them and nothing else;
 * `null` asks nothing. While asking again the last answer stays shown.
 */
export function useEachCaptureModes(
  queries: readonly (CaptureSettings | null)[],
): ModesState[] {
  const client = use(CaptureModesContext);
  if (!client) throw new Error('Outside the portal.');
  const key = JSON.stringify(queries);
  /** How often the user asked again; each time asks once more. */
  const [attempt, setAttempt] = useState(0);
  const [answer, setAnswer] = useState<{ key: string; states: ModesState[] }>();
  useEffect(() => {
    const asked = JSON.parse(key) as (CaptureSettings | null)[];
    // A newer ask answers for the hardware as it is now; leaving ends it.
    const controller = new AbortController();
    const again = () => setAttempt((count) => count + 1);
    const one = (query: CaptureSettings | null): Promise<ModesState> =>
      query
        ? client.modes(query, controller.signal).then(
            (modes) =>
              modes.status === 'known'
                ? { state: 'known', modes: modes.modes }
                : { state: 'unknown', again },
            () => ({ state: 'failed', again }),
          )
        : Promise.resolve({ state: 'loading' });
    void Promise.all(asked.map(one)).then((states) => {
      if (!controller.signal.aborted) setAnswer({ key, states });
    });
    return () => controller.abort();
  }, [client, key, attempt]);
  return answer?.key === key
    ? answer.states
    : queries.map(() => ({ state: 'loading' }));
}

/** `useEachCaptureModes` of one query. */
export function useCaptureModes(query: CaptureSettings | null): ModesState {
  const [modes = { state: 'loading' }] = useEachCaptureModes([query]);
  return modes;
}
