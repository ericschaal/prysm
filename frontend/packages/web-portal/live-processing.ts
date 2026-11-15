import { useEffect, useEffectEvent, useMemo, useRef, useState } from 'react';
import { failureOf, same } from '@prysm/application-client';
import type { Processing } from '@prysm/contracts/application';
import { useDraft } from './draft';
import type { SaveState } from './save-state';
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
export type ProcessingControls = Pick<
  Processing,
  | 'brightness_percent'
  | 'style'
  | 'custom_style'
  | 'sample_density'
  | 'remove_black_bars'
  | 'calibrations'
  | 'active_calibration'
>;

/**
 * Limits from the generated `Processing` schema (`minimum`/`maximum`),
 * checked against it in tests/portal.test.jsx. Where the schema has no
 * maximum, the Rust field type bounds it: `sample_density` is a `u16`.
 */
export const processingLimits = {
  brightness_percent: { min: 0, max: 100, step: 1 },
  sample_density: { min: 1, max: 65535, step: 1 },
} as const;

/** The style knobs' limits, from the generated `StyleValues` schema likewise. */
export const styleLimits = {
  smoothing_seconds: { min: 0, step: 'any' },
  cut_response_percent: { min: 0, max: 100, step: 1 },
  brightness_ramp_seconds: { min: 0, max: 2, step: 0.1 },
  spatial_reach_percent: { min: 1, max: 100, step: 1 },
  color_focus_percent: { min: 0, max: 100, step: 1 },
  saturation_boost_percent: { min: 0, max: 100, step: 1 },
  dark_floor_percent: { min: 0, max: 20, step: 1 },
} as const;

/**
 * Smoothing and Detail as sliders: the range a person tunes in, widened to
 * any saved value beyond it (both settings allow more than is useful).
 */
export const fineLimits = {
  smoothing: (value: number) => ({
    min: styleLimits.smoothing_seconds.min,
    max: Math.max(2, value),
    step: 0.05,
  }),
  detail: (value: number) => ({
    min: processingLimits.sample_density.min,
    max: Math.max(512, value),
    step: 1,
  }),
};

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

export function processingControls(processing: Processing): ProcessingControls {
  return withCalibrations(
    {
      brightness_percent: processing.brightness_percent,
      style: processing.style,
      // Absent until first tuned, as the server sends it.
      ...(processing.custom_style
        ? { custom_style: processing.custom_style }
        : {}),
      sample_density: processing.sample_density,
      remove_black_bars: processing.remove_black_bars,
    },
    processing.calibrations,
    processing.active_calibration,
  );
}

/**
 * `controls` with `calibrations` and `active_calibration` as the server sends them:
 * absent when empty or none, so a saved draft equals the state that follows.
 */
export function withCalibrations<T extends object>(
  controls: T,
  calibrations: Processing['calibrations'],
  active: string | undefined,
): Omit<T, 'calibrations' | 'active_calibration'> &
  Pick<Processing, 'calibrations' | 'active_calibration'> {
  const rest = { ...controls } as Partial<
    Pick<Processing, 'calibrations' | 'active_calibration'>
  >;
  delete rest.calibrations;
  delete rest.active_calibration;
  return {
    ...(rest as Omit<T, 'calibrations' | 'active_calibration'>),
    ...(calibrations?.length ? { calibrations } : {}),
    ...(active === undefined ? {} : { active_calibration: active }),
  };
}

/**
 * `draft`, moved onto `saved`: every control the user changed since `base`
 * keeps the user's value, every other one takes the saved value. A change
 * made on another device to a different control is kept; to the same one,
 * the user's own (shown) value wins.
 */
export function rebased(
  saved: ProcessingControls,
  base: ProcessingControls,
  draft: ProcessingControls,
): ProcessingControls {
  const next: Record<string, unknown> = { ...saved };
  const mine = draft as Record<string, unknown>;
  const before = base as Record<string, unknown>;
  for (const key of new Set([...Object.keys(mine), ...Object.keys(before)])) {
    if (same(mine[key], before[key])) continue;
    if (key in mine) next[key] = mine[key];
    else delete next[key];
  }
  return next as unknown as ProcessingControls;
}

/** A save that failed, shown until the draft changes or it is retried. */
interface FailedSave {
  /** The draft that failed; not saved again until it changes. */
  value: ProcessingControls;
  message: string;
  /** The control where a retry is offered, when the save may work again. */
  retryAt: string | null;
}

/**
 * The processing section as a draft that saves itself: each change is applied
 * shortly after the user stops adjusting it, and `commit` applies one at once.
 * Shared by the Picture panel, the light card and the calibration wizard, so
 * all save the same way. Render the fields inside `FieldScopeProvider
 * value={scope}`.
 *
 * A save refused because the settings changed elsewhere is not lost: once
 * the newer settings arrive, the user's changes are moved onto them and
 * saved again. A failed save is shown at the control that was changed, with
 * a retry there.
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
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<FailedSave | null>(null);
  /** The control last changed, where a failed save is shown. */
  const changedRef = useRef('');
  /** Refused as stale at this revision: wait for newer settings. */
  const [staleAt, setStaleAt] = useState<number | null>(null);
  /** A slider is held: its value is tried on the lights and saved on release. */
  const [held, setHeld] = useState(false);
  // Also as a ref: a key's change follows its keydown before a render.
  const holding = useRef(false);
  const [trials] = useState(() => throttled<Processing>(trialIntervalMs));
  /** Values were tried since the last save, so the lights may not match it. */
  const [tried, setTried] = useState(false);
  /** A field's own check or the server refuses a value. */
  const invalid =
    Object.keys(errors.local).length > 0 ||
    Object.keys(errors.server).length > 0;
  const blocked =
    disabled ||
    busy ||
    draft.syncing ||
    staleAt === revision ||
    invalid ||
    (failed !== null && same(failed.value, draft.draft));

  // `value` decides the calibrations: an absent one was removed.
  const whole = (value: ProcessingControls): Processing => ({
    ...withCalibrations(processing, undefined, undefined),
    ...value,
  });

  /**
   * The save on its way, if any; it resolves with whether it was saved. Saves never overlap: one asked for meanwhile
   * (a quick second tap) waits for it, then the newest value is saved
   * against the revision it returned, at once.
   */
  const inFlight = useRef<Promise<boolean> | null>(null);
  const again = useRef(false);
  function apply(edited: ProcessingControls): Promise<boolean> {
    if (inFlight.current) {
      again.current = true;
      return inFlight.current;
    }
    const run = send(edited).finally(() => {
      inFlight.current = null;
    });
    inFlight.current = run;
    return run;
  }
  async function send(edited: ProcessingControls): Promise<boolean> {
    let value = edited;
    // Changed elsewhere since the draft began: keep both changes.
    if (draft.conflicted && draft.base) {
      value = rebased(saved, draft.base, edited);
      draft.rebase(value);
    }
    setBusy(true);
    setStaleAt(null);
    try {
      // A trial still on its way must not land after the save.
      await trials.settle();
      setTried(false);
      const next = await save(revision, whole(value));
      draft.saved(value, next);
      setFailed(null);
      return true;
    } catch (error) {
      const reason = failureOf(error);
      // Saved again once the newer settings arrive.
      if (reason.kind === 'stale_revision') {
        setStaleAt(revision);
        return false;
      }
      const invalid = reason.kind === 'invalid';
      const at = (invalid && reason.field) || changedRef.current;
      if (at) errors.setServer({ [at]: invalid ? refusedValue : 'Not saved.' });
      setFailed({
        value,
        message: saveProblem(reason),
        retryAt: at && !invalid ? at : null,
      });
      return false;
    } finally {
      setBusy(false);
    }
  }
  // Debounce: every change restarts the wait. A new revision (another section
  // saved meanwhile) also restarts it, so `apply` sends the current one.
  const applyLater = useEffectEvent((value: ProcessingControls) => {
    again.current = false;
    void apply(value);
  });
  // A held slider saves on release instead. Values tried on the lights are
  // saved too, even when they ended where they started, to end the trial. A
  // save asked for while another was on its way goes without the wait.
  const pending = draft.dirty || tried;
  useEffect(() => {
    if (!pending) again.current = false;
    if (!pending || blocked || held) return;
    const timer = setTimeout(
      () => applyLater(draft.draft),
      again.current ? 0 : processingDelayMs,
    );
    return () => clearTimeout(timer);
  }, [draft.draft, pending, blocked, held, revision]);

  const { clearServer } = errors;
  const retryAt = failed?.retryAt ?? null;
  const retry = useMemo(
    () =>
      retryAt === null
        ? null
        : {
            path: retryAt,
            run: () => {
              clearServer();
              setFailed(null);
            },
          },
    [retryAt, clearServer],
  );
  const scope = useFieldScope(disabled, errors, true, retry);

  /** Change the draft; `path` names the control, for where a failure shows. */
  function replace(next: ProcessingControls, path?: string) {
    if (path) changedRef.current = path;
    draft.setDraft(next);
  }
  let state: SaveState = 'idle';
  if (failed || invalid) state = 'failed';
  else if (busy || draft.syncing || draft.dirty) state = 'saving';
  return {
    value: draft.draft,
    /** Changes when the draft is replaced from outside, to remount inputs. */
    generation: draft.generation,
    dirty: draft.dirty,
    /** A save is on its way. */
    saving: busy || draft.syncing,
    /** For the page's save status: changes apply by themselves. */
    state,
    failure: failed?.message ?? '',
    /** Try the failed save again, if it may work. */
    retry: retry?.run,
    scope,
    replace,
    /** Change the draft and apply it now (pending changes included). */
    commit: (next: ProcessingControls, path?: string) => {
      replace(next, path);
      return apply(next);
    },
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
