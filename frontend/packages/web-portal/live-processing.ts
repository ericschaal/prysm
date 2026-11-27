import {
  useEffect,
  useEffectEvent,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { failureOf, same } from '@prysm/application-client';
import type {
  Calibration,
  CorrectionControls,
} from '@prysm/contracts/application';
import type { Processing } from '@prysm/contracts/application-reading';
import { processingBounds, styleBounds } from '@prysm/contracts/constants';
import { useDraft } from './draft';
import { usePortal } from './portal-context';
import {
  refusedValue,
  saveProblem,
  useFieldErrors,
  useFieldScope,
} from './form';

/**
 * What this panel owns: brightness, the style and the user's own style
 * values, detail, black bars, plus the calibrations and the active one.
 * Other processing fields are preserved.
 */
export type ProcessingControls = ReturnType<typeof pictureControls> &
  Pick<Processing, 'calibrations' | 'active_calibration'>;

export const processingLimits = {
  brightness_percent: { ...processingBounds.brightness_percent, step: 1 },
  sample_density: { ...processingBounds.sample_density, step: 1 },
} as const;

export const styleLimits = {
  smoothing_seconds: { ...styleBounds.smoothing_seconds, step: 0.05 },
  cut_response_percent: { ...styleBounds.cut_response_percent, step: 1 },
  brightness_ramp_seconds: {
    ...styleBounds.brightness_ramp_seconds,
    step: 0.1,
  },
  spatial_reach_percent: { ...styleBounds.spatial_reach_percent, step: 1 },
  color_focus_percent: { ...styleBounds.color_focus_percent, step: 1 },
  saturation_boost_percent: {
    ...styleBounds.saturation_boost_percent,
    step: 1,
  },
  dark_floor_percent: { ...styleBounds.dark_floor_percent, step: 1 },
} as const;

/** How long “Saved” shows before the page says all changes are saved. */
export const savedForMs = 2000;

/** Wait this long after the last change before applying it. */
export const processingDelayMs = 400;

/** While a slider is held, try its value on the lights at most this often. */
export const trialIntervalMs = 100;

/**
 * Sends at most one value every `intervalMs`, one request at a time, the
 * latest value winning: the first push goes at once, later ones wait for the
 * interval and for the request before them, and only the newest of them is
 * sent. Failures are ignored; the final value is saved separately.
 */
export function throttled<T>(intervalMs: number) {
  let pending: { value: T; send: (value: T) => Promise<unknown> } | undefined;
  let inFlight: Promise<void> | undefined;
  let last = -Infinity;
  let timer: ReturnType<typeof setTimeout> | undefined;
  function pump() {
    if (inFlight || !pending) return;
    const wait = last + intervalMs - Date.now();
    if (wait > 0) {
      timer ??= setTimeout(() => {
        timer = undefined;
        pump();
      }, wait);
      return;
    }
    const { value, send } = pending;
    pending = undefined;
    last = Date.now();
    // A failure is dropped like a success: the next value follows.
    const next = () => {
      inFlight = undefined;
      pump();
    };
    inFlight = send(value).then(next, next);
  }
  return {
    push(value: T, send: (value: T) => Promise<unknown>) {
      pending = { value, send };
      pump();
    },
    /** Drop what waits to be sent; resolves once nothing is in flight. */
    async settle() {
      pending = undefined;
      clearTimeout(timer);
      timer = undefined;
      while (inFlight) await inFlight;
    },
  };
}

function pictureControls(processing: Processing) {
  return {
    brightness_percent: processing.brightness_percent,
    style: processing.style,
    ...(processing.custom_style
      ? { custom_style: processing.custom_style }
      : {}),
    sample_density: processing.sample_density,
    remove_black_bars: processing.remove_black_bars,
  };
}

export function processingControls(processing: Processing): ProcessingControls {
  return withCalibrations(
    pictureControls(processing),
    processing.calibrations,
    processing.active_calibration,
  );
}

/**
 * `controls` with `calibrations` and `active_calibration` as the server sends them:
 * absent when empty or none, so a saved draft equals the state that follows.
 */
export function withCalibrations(
  controls: ProcessingControls,
  calibrations: Processing['calibrations'],
  active: string | undefined,
): ProcessingControls {
  const next = { ...controls };
  if (calibrations?.length) next.calibrations = calibrations;
  else delete next.calibrations;
  if (active === undefined) delete next.active_calibration;
  else next.active_calibration = active;
  return next;
}

/**
 * `draft`, moved onto `saved`: every control the user changed since `base`
 * keeps the user's value, every other one takes the saved value. A change
 * made on another device to a different control is kept; to the same one,
 * the user's own (shown) value wins. Each calibration counts as a control of
 * its own (see {@link rebasedCalibrations}).
 */
export function rebased<T extends ProcessingControls>(
  saved: T,
  base: T,
  draft: T,
): ProcessingControls {
  const next = { ...saved };
  const fields = { ...draft, ...base };
  for (const key in fields) {
    if (!Object.prototype.hasOwnProperty.call(fields, key)) continue;
    if (key === 'calibrations' || same(draft[key], base[key])) continue;
    if (key in draft) next[key] = draft[key];
    else delete next[key];
  }
  const calibrations = rebasedCalibrations(
    saved.calibrations ?? [],
    base.calibrations ?? [],
    draft.calibrations ?? [],
  );
  if (calibrations.length) next.calibrations = calibrations;
  else delete next.calibrations;
  return next;
}

/**
 * `draft`'s calibrations moved onto `saved`'s, one calibration at a time by
 * its `id`: one the user changed keeps the user's version, one the user
 * added is appended, one deleted on either side is gone, and every other
 * one is as saved, in the saved order.
 */
export function rebasedCalibrations(
  saved: readonly Calibration[],
  base: readonly Calibration[],
  draft: readonly Calibration[],
): Calibration[] {
  const before = new Map(
    base.map((calibration) => [calibration.id, calibration]),
  );
  const mine = new Map(
    draft.map((calibration) => [calibration.id, calibration]),
  );
  const merged: Calibration[] = [];
  for (const theirs of saved) {
    const original = before.get(theirs.id);
    const edited = mine.get(theirs.id);
    if (!original) merged.push(edited ?? theirs);
    else if (edited) merged.push(same(edited, original) ? theirs : edited);
  }
  for (const added of draft) {
    if (!before.has(added.id) && !saved.some((item) => item.id === added.id))
      merged.push(added);
  }
  return merged;
}

export type SaveState = 'idle' | 'saving' | 'failed';

/** A save that failed, shown until the draft changes or it is retried. */
interface FailedSave {
  /** The draft that failed; not saved again until it changes. */
  value: ProcessingControls;
  message: string;
  /** The control where a retry is offered, when the save may work again. */
  retryAt: string | null;
  /** The revision the state showed when it was sent. */
  sentAt: number;
}

/**
 * The last save: none, or the last one answered (`idle`); one on its way;
 * refused as stale at revision `at` (sent again once newer settings
 * arrive); or failed.
 */
type SavePhase =
  | { kind: 'idle' }
  | { kind: 'saving' }
  | { kind: 'stale'; at: number }
  | ({ kind: 'failed' } & FailedSave);

const idle: SavePhase = { kind: 'idle' };

/**
 * Edit processing settings with debounced saves and live slider trials.
 * Commits run in order and return the value saved after any rebase, or null
 * on failure. Field errors and retry actions are exposed through `scope`.
 */
export function useLiveProcessing({
  processing,
  revision,
  disabled,
  save,
  tryProcessing,
}: {
  processing: Processing;
  revision: number;
  disabled: boolean;
  /** Save the whole processing section against `expected`. */
  save: (expected: number, processing: Processing) => Promise<number>;
  /** Light with a processing section unsaved, while a slider is held. */
  tryProcessing?: (processing: Processing) => Promise<unknown>;
}) {
  const saved = processingControls(processing);
  const draft = useDraft<ProcessingControls>({
    saved,
    revision,
    fresh: () => saved,
  });
  const errors = useFieldErrors();
  const [phase, setPhase] = useState<SavePhase>(idle);
  /** The control last changed, where a failed save is shown. */
  const changedRef = useRef('');
  /** A slider is held: its value is tried on the lights and saved on release. */
  const [held, setHeld] = useState(false);
  // Also as a ref: a key's change follows its keydown before a render.
  const holding = useRef(false);
  const [trials] = useState(() => throttled<Processing>(trialIntervalMs));
  /** Values were tried since the last save, so the lights may not match it. */
  const [tried, setTried] = useState(false);
  if (
    phase.kind === 'failed' &&
    revision !== phase.sentAt &&
    same(phase.value, saved)
  ) {
    setPhase(idle);
    errors.clearServer();
  }
  const invalid =
    Object.keys(errors.local).length > 0 ||
    Object.keys(errors.server).length > 0;
  /** A save is on its way, or the state has not shown the last one yet. */
  const saving = phase.kind === 'saving' || draft.syncing;
  const blocked =
    disabled ||
    saving ||
    (phase.kind === 'stale' && phase.at === revision) ||
    invalid ||
    (phase.kind === 'failed' && same(phase.value, draft.draft));

  /** Queued saves read the most recently committed render. */
  const latest = useRef({ processing, revision, saved, draft });
  useLayoutEffect(() => {
    latest.current = { processing, revision, saved, draft };
  });
  // `value` decides the calibrations: an absent one was removed.
  const whole = (value: ProcessingControls): Processing => ({
    ...withCalibrations(latest.current.processing, undefined, undefined),
    ...value,
  });

  /** The most recently queued save. */
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  /** Acknowledged revision to use until the stream advances. */
  const lastSave = useRef<{ sentAt: number; savedAs: number } | null>(null);
  /**
   * Save `edited` after the saves before it; resolves with what was saved
   * (`edited`, moved onto changes made elsewhere), or `null`.
   */
  function apply(
    edited: ProcessingControls,
  ): Promise<ProcessingControls | null> {
    const run = queue.current.then(() => send(edited));
    queue.current = run;
    return run;
  }
  async function send(
    edited: ProcessingControls,
  ): Promise<ProcessingControls | null> {
    const current = latest.current;
    const last = lastSave.current;
    const expected =
      last && last.sentAt === current.revision
        ? last.savedAs
        : current.revision;
    let value = edited;
    // Changed elsewhere since the draft began: keep both changes.
    if (current.draft.conflicted && current.draft.base) {
      value = rebased(current.saved, current.draft.base, edited);
      current.draft.rebase(value);
    }
    errors.clearServer();
    setPhase({ kind: 'saving' });
    try {
      // A trial still on its way must not land after the save.
      await trials.settle();
      setTried(false);
      const next = await save(expected, whole(value));
      lastSave.current = { sentAt: current.revision, savedAs: next };
      current.draft.saved(value, next);
      setPhase(idle);
      return value;
    } catch (error) {
      const reason = failureOf(error);
      if (reason.kind === 'stale_revision') {
        setPhase({ kind: 'stale', at: expected });
        return null;
      }
      const invalid = reason.kind === 'invalid';
      const at = (invalid && reason.field) || changedRef.current;
      if (at) errors.setServer({ [at]: invalid ? refusedValue : 'Not saved.' });
      setPhase({
        kind: 'failed',
        value,
        message: saveProblem(reason),
        retryAt: at && !invalid ? at : null,
        sentAt: current.revision,
      });
      return null;
    }
  }
  // Debounce: every change restarts the wait. A new revision (another section
  // saved meanwhile) also restarts it.
  const applyLater = useEffectEvent(apply);
  // A held slider saves on release instead. Values tried on the lights are
  // saved too, even when they ended where they started, to end the trial.
  const pending = draft.dirty || tried;
  useEffect(() => {
    if (!pending || blocked || held) return;
    const timer = setTimeout(
      () => void applyLater(draft.draft),
      processingDelayMs,
    );
    return () => clearTimeout(timer);
  }, [draft.draft, pending, blocked, held, revision]);

  const { clearServer } = errors;
  const retryAt = phase.kind === 'failed' ? phase.retryAt : null;
  const retry = useMemo(
    () =>
      retryAt === null
        ? null
        : {
            path: retryAt,
            run: () => {
              clearServer();
              setPhase((now) => (now.kind === 'failed' ? idle : now));
            },
          },
    [retryAt, clearServer, setPhase],
  );
  const scope = useFieldScope(disabled, errors, true, retry);

  /** Change the draft; `path` names the control, for where a failure shows. */
  function replace(next: ProcessingControls, path?: string) {
    if (path) changedRef.current = path;
    draft.setDraft(next);
    // Another value answers a failed save, and is saved in its turn; a
    // refused value stays marked until its field is edited.
    if (phase.kind === 'failed' && !same(phase.value, next)) {
      if (phase.retryAt !== null) clearServer();
      setPhase((now) =>
        now.kind === 'failed' && !same(now.value, next) ? idle : now,
      );
    }
  }
  let state: SaveState = 'idle';
  if (phase.kind === 'failed' || invalid) state = 'failed';
  else if (saving || draft.dirty) state = 'saving';
  return {
    value: draft.draft,
    /** Changes when the draft is replaced from outside, to remount inputs. */
    generation: draft.generation,
    dirty: draft.dirty,
    /** A save is on its way. */
    saving,
    /** For the page's save status: changes apply by themselves. */
    state,
    failure: phase.kind === 'failed' ? phase.message : '',
    /** Try the failed save again, if it may work. */
    retry: retry?.run,
    scope,
    replace,
    /**
     * Change the draft and save it after the saves on their way; resolves
     * with what was saved of `next` (see `apply`), or `null`.
     */
    commit: (next: ProcessingControls, path?: string) => {
      replace(next, path);
      return apply(next);
    },
    /** The draft as the latest render shows it, for a change made after a wait. */
    current: () => latest.current.draft.draft,
    /** Discard the draft for the saved section as the latest render shows it. */
    reload: () => latest.current.draft.reload(),
    /**
     * A slider: `press` when the pointer or a key goes down on it, `slide`
     * for each value (tried on the lights, throttled), `release` when it
     * comes up, which saves the final value. Without `tryProcessing` it is
     * an ordinary change, saved after the usual wait.
     */
    slider: {
      press: () => {
        if (!tryProcessing || disabled) return;
        holding.current = true;
        setHeld(true);
      },
      slide: (next: ProcessingControls, path?: string) => {
        replace(next, path);
        if (!tryProcessing || !holding.current) return;
        setTried(true);
        trials.push(whole(next), tryProcessing);
      },
      release: () => {
        if (!holding.current) return;
        holding.current = false;
        setHeld(false);
        if (draft.dirty || tried) void apply(draft.draft);
      },
    },
  };
}

/** A processing draft bound to the connected portal. */
export function useProcessingEditor() {
  const { snapshot, online, commands } = usePortal();
  return useLiveProcessing({
    processing: snapshot.settings.processing,
    revision: snapshot.revision,
    disabled: !online,
    save: commands.configureProcessing,
    tryProcessing: commands.tryProcessing,
  });
}

export type ProcessingEditor = ReturnType<typeof useProcessingEditor>;

/** Edit one calibration in the current draft. Both editors address field errors by its current list index. */
export function editCalibration(
  live: ProcessingEditor,
  id: string | undefined,
) {
  const calibrations = live.value.calibrations ?? [];
  const index = calibrations.findIndex((item) => item.id === id);
  const calibration = calibrations[index];
  if (!calibration) return null;
  const path = `processing.calibrations[${index}]`;
  function update(change: Partial<Calibration>, field: string) {
    const next = calibrations.map((item, at) =>
      at === index ? { ...item, ...change } : item,
    );
    live.slider.slide(
      withCalibrations(live.value, next, live.value.active_calibration),
      `${path}.${field}`,
    );
  }
  const { correction } = calibration;
  const sliders = {
    path: `${path}.correction`,
    values: correction,
    change: (patch: Partial<CorrectionControls>, control: string) =>
      update(
        { correction: { ...correction, ...patch } },
        `correction.${control}`,
      ),
    onPress: live.slider.press,
    onRelease: live.slider.release,
  };
  return { calibration, index, path, sliders, update };
}
