import { useEffect, useId, useState } from 'react';
import type { Processing } from '@prysm/contracts/application';
import { failureOf, same } from './adapters';
import { useDraft } from './draft';
import {
  CheckboxField,
  FieldScopeProvider,
  NumberField,
  RangeField,
  useFieldErrors,
  useFieldScope,
} from './fields';
import { staleNotice } from './section-form';

/** The six controls this panel owns; other processing fields are preserved. */
export type ProcessingControls = Pick<
  Processing,
  | 'brightness_percent'
  | 'shadow_lift_percent'
  | 'smoothing_seconds'
  | 'sample_density'
  | 'spatial_reach_percent'
  | 'remove_black_bars'
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
  return {
    brightness_percent: processing.brightness_percent,
    shadow_lift_percent: processing.shadow_lift_percent,
    smoothing_seconds: processing.smoothing_seconds,
    sample_density: processing.sample_density,
    spatial_reach_percent: processing.spatial_reach_percent,
    remove_black_bars: processing.remove_black_bars,
  };
}

const percent = (value: number) => `${value}%`;

/**
 * Picture controls apply live: processing-only changes reach a running
 * session at the next frame without restarting lighting, so each change is
 * saved shortly after the user stops adjusting it.
 */
export function ProcessingPanel({
  processing,
  revision,
  disabled,
  running,
  save,
}: {
  processing: Processing;
  revision: number;
  disabled: boolean;
  running: boolean;
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
  const headingId = useId();
  const scope = useFieldScope(disabled, errors, true);
  const blocked =
    disabled ||
    busy ||
    draft.syncing ||
    Object.keys(errors.local).length > 0 ||
    Object.keys(errors.server).length > 0 ||
    (failed !== null && same(failed, draft.draft));

  async function apply() {
    if (draft.conflicted) {
      draft.reload();
      setNotice(staleNotice);
      return;
    }
    const value = draft.draft;
    setBusy(true);
    try {
      const next = await save(revision, { ...processing, ...value });
      draft.saved(value, next);
      setFailure('');
      setFailed(null);
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
    } finally {
      setBusy(false);
    }
  }
  // Debounce: every change restarts the wait. A new revision (another section
  // saved meanwhile) also restarts it, so `apply` sends the current one.
  useEffect(() => {
    if (!draft.dirty || blocked) return;
    const timer = setTimeout(() => void apply(), processingDelayMs);
    return () => clearTimeout(timer);
  }, [draft.draft, draft.dirty, blocked, revision]);

  function change(next: Partial<ProcessingControls>) {
    draft.setDraft({ ...draft.draft, ...next });
    setNotice('');
  }
  const value = draft.draft;
  let status = running ? 'Applied to your lights' : 'Saved; used when screen sync runs';
  if (busy || draft.syncing) status = 'Applying…';
  else if (draft.dirty) status = failure ? 'Not applied' : 'Waiting to apply…';
  return (
    <section className="card processing" aria-labelledby={headingId}>
      <div className="card-heading">
        <h2 id={headingId}>Picture</h2>
        <span
          className={`badge${draft.dirty ? ' attention' : ''}`}
          role="status"
        >
          {status}
        </span>
      </div>
      <p className="intro">
        Changes apply as you adjust them, without restarting screen sync.
      </p>
      <FieldScopeProvider value={scope}>
        <div key={draft.generation} className="grid">
          <RangeField
            path="processing.brightness_percent"
            label="Brightness"
            help="Output brightness of the lights."
            value={value.brightness_percent}
            onChange={(brightness_percent) => change({ brightness_percent })}
            format={percent}
            {...processingLimits.brightness_percent}
          />
          <RangeField
            path="processing.shadow_lift_percent"
            label="Shadow lift"
            help="Lift dark colours while keeping black off."
            value={value.shadow_lift_percent}
            onChange={(shadow_lift_percent) => change({ shadow_lift_percent })}
            format={percent}
            {...processingLimits.shadow_lift_percent}
          />
          <RangeField
            path="processing.spatial_reach_percent"
            label="Spatial reach"
            help="How far into the picture each LED samples from its edge."
            value={value.spatial_reach_percent}
            onChange={(spatial_reach_percent) =>
              change({ spatial_reach_percent })
            }
            format={percent}
            {...processingLimits.spatial_reach_percent}
          />
          <NumberField
            path="processing.smoothing_seconds"
            label="Smoothing"
            unit="seconds"
            help="Time to soften a colour change. 0 turns smoothing off."
            value={value.smoothing_seconds}
            onChange={(smoothing_seconds) => change({ smoothing_seconds })}
            {...processingLimits.smoothing_seconds}
          />
          <NumberField
            path="processing.sample_density"
            label="Sample density"
            unit="per 1000 pixels"
            help="Higher values follow detail more closely and use more CPU. 128 is a good start."
            value={value.sample_density}
            onChange={(sample_density) => change({ sample_density })}
            {...processingLimits.sample_density}
          />
          <CheckboxField
            path="processing.remove_black_bars"
            label="Remove black bars"
            help="Detect the active picture before sampling colours."
            checked={value.remove_black_bars}
            onChange={(remove_black_bars) => change({ remove_black_bars })}
          />
        </div>
      </FieldScopeProvider>
      {notice ? (
        <p className="notice warning" role="status">
          {notice}
        </p>
      ) : null}
      {failure ? (
        <p className="notice error" role="alert">
          {failure}
        </p>
      ) : null}
    </section>
  );
}
