import { useEffect, useEffectEvent, useState } from 'react';
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
}: {
  processing: Processing;
  revision: number;
  disabled: boolean;
  /** Save the whole processing section against `expected`. */
  save: (expected: number, processing: Processing) => Promise<number>;
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
  const blocked =
    disabled ||
    busy ||
    draft.syncing ||
    Object.keys(errors.local).length > 0 ||
    Object.keys(errors.server).length > 0 ||
    (failed !== null && same(failed, draft.draft));

  /** Save `value`; resolves with whether it was saved. */
  async function apply(value: ProcessingControls): Promise<boolean> {
    if (draft.conflicted) {
      draft.reload();
      setNotice(staleNotice);
      return false;
    }
    setBusy(true);
    try {
      // `value` decides the profiles: an absent one was removed.
      const next = await save(revision, {
        ...withProfiles(processing, undefined, undefined),
        ...value,
      });
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
  useEffect(() => {
    if (!draft.dirty || blocked) return;
    const timer = setTimeout(() => applyLater(draft.draft), processingDelayMs);
    return () => clearTimeout(timer);
  }, [draft.draft, draft.dirty, blocked, revision]);

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
  };
}
