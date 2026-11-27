import { useEffect, useEffectEvent, useState } from 'react';
import type { CaptureSettings, Mode } from '@prysm/application-client';
import { useHost } from './portal-context';
import type { Picture } from './adapters';
import {
  picturePresets,
  presetTargets,
  type PicturePreset,
} from './picture-presets';

/** An unanswered probe can be retried. A known source may offer no presets. */
export type ModesState =
  | { state: 'loading' }
  | { state: 'unknown'; again: () => void }
  | { state: 'failed'; again: () => void }
  | { state: 'known'; modes: readonly Mode[]; offered: readonly Offered[] };

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

function preferred(candidate: Size, current: Size) {
  const wide = candidate.width * 9 === candidate.height * 16;
  const currentWide = current.width * 9 === current.height * 16;
  if (wide !== currentWide) return wide;
  return candidate.width * candidate.height < current.width * current.height;
}

/**
 * What `modes` deliver for `target`: the target itself where a mode takes
 * it, else the smallest size (16:9 first) at least as large at the same
 * rate; `null` when nothing covers it.
 */
export function resolve(modes: readonly Mode[], target: Size): Size | null {
  if (delivers(modes, target)) return target;
  let best: Size | null = null;
  for (const mode of modes) {
    if (!contains(mode.fps, target.fps)) continue;
    const width = atLeast(mode.width, target.width);
    const height = atLeast(mode.height, target.height);
    if (width === null || height === null) continue;
    const candidate = { width, height, fps: target.fps };
    if (!best || preferred(candidate, best)) best = candidate;
  }
  return best;
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
  const offered: Offered[] = [];
  for (const { preset } of presetTargets) {
    const size = resolve(modes, picturePresets[preset]);
    if (!size || offered.some((other) => sameSize(other, size))) continue;
    offered.push({ preset, ...size });
  }
  return offered;
}

/** Whether `modes` leave nothing to choose: no mode, or none for a preset. */
export const nothingOffered = (modes: ModesState) =>
  modes.state === 'known' && modes.offered.length === 0;

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
  const client = useHost().captureModes;
  const key = JSON.stringify(queries);
  const currentQueries = useEffectEvent(() => queries);
  const [attempt, setAttempt] = useState(0);
  const [answer, setAnswer] = useState<{ key: string; states: ModesState[] }>();
  useEffect(() => {
    const controller = new AbortController();
    const again = () => setAttempt((count) => count + 1);
    const one = (query: CaptureSettings | null): Promise<ModesState> =>
      query
        ? client.modes(query, controller.signal).then(
            (modes) =>
              modes.status === 'known'
                ? {
                    state: 'known',
                    modes: modes.modes,
                    offered: offeredPresets(modes.modes),
                  }
                : { state: 'unknown', again },
            () => ({ state: 'failed', again }),
          )
        : Promise.resolve({ state: 'loading' });
    void Promise.all(currentQueries().map(one)).then((states) => {
      if (!controller.signal.aborted) setAnswer({ key, states });
    });
    return () => controller.abort();
  }, [client, key, attempt]);
  return answer?.key === key
    ? answer.states
    : queries.map(() => ({ state: 'loading' }));
}

export function useCaptureModes(query: CaptureSettings | null): ModesState {
  const [modes = { state: 'loading' }] = useEachCaptureModes([query]);
  return modes;
}
