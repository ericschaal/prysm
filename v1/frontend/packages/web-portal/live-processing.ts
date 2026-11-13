import { useEffect, useEffectEvent, useRef, useState } from 'react';
import { failureOf, same } from '@prysm/application-client';
import type { Processing } from '@prysm/contracts/application';
import { useDraft } from './draft';
import { staleNotice, useFieldErrors, useFieldScope } from './form';

/**
 * What this panel owns: six controls plus the colour profiles and the active
 * one. Other processing fields are preserved.
 */
export type ProcessingControls = Pick<
  Processing,
  | 'brightness_percent'
  | 'shadow_lift_percent'
  | 'smoothing_seconds'
  | 'sample_density'
  | 'spatial_reach_percent'
  | 'remove_black_bars'
  | 'profiles'
  | 'active_profile'
>;

/**
 * Limits from the generated `Processing` schema (`minimum`/`maximum`),
 * checked against it in tests/contracts.test.mjs. Where the schema has no
 * maximum, the Rust field type bounds it: `sample_density` is a `u16`.
 */
export const processingLimits = {
  brightness_percent: { min: 0, max: 100, step: 1 },
  shadow_lift_percent: { min: 0, max: 100, step: 1 },
  spatial_reach_percent: { min: 1, max: 100, step: 1 },
  smoothing_seconds: { min: 0, step: 'any' },
  sample_density: { min: 1, max: 65535, step: 1 },
} as const;

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
    inFlight = send(value).then(
      () => {
        inFlight = undefined;
        pump();
      },
      () => {
        inFlight = undefined;
        pump();
      },
    );
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
  return withProfiles(
    {
      brightness_percent: processing.brightness_percent,
      shadow_lift_percent: processing.shadow_lift_percent,
      smoothing_seconds: processing.smoothing_seconds,
      sample_density: processing.sample_density,
      spatial_reach_percent: processing.spatial_reach_percent,
      remove_black_bars: processing.remove_black_bars,
    },
    processing.profiles,
    processing.active_profile,
  );
}

/**
 * `controls` with `profiles` and `active_profile` as the server sends them:
 * absent when empty or none, so a saved draft equals the state that follows.
 */
export function withProfiles<T extends object>(
  controls: T,
  profiles: Processing['profiles'],
  active: string | undefined,
): Omit<T, 'profiles' | 'active_profile'> &
  Pick<Processing, 'profiles' | 'active_profile'> {
  const rest = { ...controls } as Partial<
    Pick<Processing, 'profiles' | 'active_profile'>
  >;
  delete rest.profiles;
  delete rest.active_profile;
  return {
    ...(rest as Omit<T, 'profiles' | 'active_profile'>),
    ...(profiles?.length ? { profiles } : {}),
    ...(active === undefined ? {} : { active_profile: active }),
  };
}

/**
 * The processing section as a draft that saves itself: each change is applied
 * shortly after the user stops adjusting it, and `commit` applies one at once.
 * Shared by the Picture panel and the calibration wizard, so both save the
 * same way. Render the fields inside `FieldScopeProvider value={scope}`.
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
  const [failure, setFailure] = useState('');
  /** The draft that last failed; not retried until it changes. */
  const [failed, setFailed] = useState<ProcessingControls | null>(null);
  const [notice, setNotice] = useState('');
  const scope = useFieldScope(disabled, errors, true);
  /** A slider is held: its value is tried on the lights and saved on release. */
  const [held, setHeld] = useState(false);
  // Also as a ref: a key's change follows its keydown before a render.
  const holding = useRef(false);
  const [trials] = useState(() => throttled<Processing>(trialIntervalMs));
  /** Values were tried since the last save, so the lights may not match it. */
  const [tried, setTried] = useState(false);
  const blocked =
    disabled ||
    busy ||
    draft.syncing ||
    Object.keys(errors.local).length > 0 ||
    Object.keys(errors.server).length > 0 ||
    (failed !== null && same(failed, draft.draft));

  // `value` decides the profiles: an absent one was removed.
  const whole = (value: ProcessingControls): Processing => ({
    ...withProfiles(processing, undefined, undefined),
    ...value,
  });

  /** Save `value`; resolves with whether it was saved. */
  async function apply(value: ProcessingControls): Promise<boolean> {
    if (draft.conflicted) {
      draft.reload();
      setNotice(staleNotice);
      return false;
    }
    setBusy(true);
    try {
      // A trial still on its way must not land after the save.
      await trials.settle();
      setTried(false);
      const next = await save(revision, whole(value));
      draft.saved(value, next);
      setFailure('');
      setFailed(null);
      return true;
    } catch (error) {
      const reason = failureOf(error);
      if (reason.kind === 'stale_revision') {
        draft.reload();
        setNotice(staleNotice);
      } else {
        if (reason.kind === 'invalid' && reason.field)
          errors.setServer({ [reason.field]: reason.message });
        setFailed(value);
        setFailure(`Not applied: ${reason.message}`);
      }
      return false;
    } finally {
      setBusy(false);
    }
  }
  // Debounce: every change restarts the wait. A new revision (another section
  // saved meanwhile) also restarts it, so `apply` sends the current one.
  const applyLater = useEffectEvent((value: ProcessingControls) => {
    void apply(value);
  });
  // A held slider saves on release instead.
  useEffect(() => {
    if (!draft.dirty || blocked || held) return;
    const timer = setTimeout(() => applyLater(draft.draft), processingDelayMs);
    return () => clearTimeout(timer);
  }, [draft.draft, draft.dirty, blocked, held, revision]);

  function replace(next: ProcessingControls) {
    draft.setDraft(next);
    setNotice('');
  }
  return {
    value: draft.draft,
    /** Changes when the draft is replaced from outside, to remount inputs. */
    generation: draft.generation,
    dirty: draft.dirty,
    /** A save is on its way. */
    saving: busy || draft.syncing,
    failure,
    notice,
    scope,
    /** Change the draft; it is applied after a short wait. */
    replace,
    /** Change the draft and apply it now (pending changes included). */
    commit: (next: ProcessingControls) => {
      replace(next);
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
      slide: (next: ProcessingControls) => {
        replace(next);
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
