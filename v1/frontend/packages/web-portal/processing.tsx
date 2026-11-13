import { useId } from 'react';
import type { Processing } from '@prysm/contracts/application';
import { FieldScopeProvider } from './form';
import { CheckboxField, NumberField, RangeField } from './fields';
import {
  processingLimits,
  useLiveProcessing,
  withProfiles,
  type ProcessingControls,
} from './live-processing';
import { ProfileControls } from './profiles';

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
  const live = useLiveProcessing({ processing, revision, disabled, save });
  const { failure, notice, scope, replace } = live;
  const headingId = useId();
  function change(next: Partial<ProcessingControls>) {
    replace({ ...live.value, ...next });
  }
  const value = live.value;
  let status = running
    ? 'Applied to your lights'
    : 'Saved; used when screen sync runs';
  if (live.saving) status = 'Applying…';
  else if (live.dirty) status = failure ? 'Not applied' : 'Waiting to apply…';
  return (
    <section className="card processing" aria-labelledby={headingId}>
      <div className="card-heading">
        <h2 id={headingId}>Picture</h2>
        <span
          className={`badge${live.dirty ? ' attention' : ''}`}
          role="status"
        >
          {status}
        </span>
      </div>
      <p className="intro">
        Changes apply as you adjust them, without restarting screen sync.
      </p>
      <FieldScopeProvider value={scope}>
        <div key={live.generation} className="grid">
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
        <ProfileControls
          key={`profiles-${live.generation}`}
          profiles={value.profiles ?? []}
          active={value.active_profile}
          onChange={(profiles, active) =>
            replace(withProfiles(live.value, profiles, active))
          }
        />
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
