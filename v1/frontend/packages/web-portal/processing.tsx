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
import { nameOf } from './profile-model';
import { ProfileControls } from './profiles';

const percent = (value: number) => `${value}%`;

interface PictureProps {
  processing: Processing;
  revision: number;
  disabled: boolean;
  /** Save the whole processing section against `expected`. */
  save: (expected: number, processing: Processing) => Promise<number>;
}

/** What went wrong applying a change, under the controls. */
function Problems({ notice, failure }: { notice: string; failure: string }) {
  return (
    <>
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
    </>
  );
}

/**
 * The everyday screen sync controls in the light card: brightness, and the
 * colour profiles as one-tap chips. Dragging brightness changes the lights as
 * it moves (unsaved) and saves where it is let go; a chip applies at once.
 */
export function QuickPicture({
  processing,
  revision,
  disabled,
  save,
  calibrate,
  tryProcessing,
}: PictureProps & {
  /** Opens the calibration, offered while there are no profiles. */
  calibrate: () => void;
  /** Light with a processing section unsaved, while brightness is dragged. */
  tryProcessing: (processing: Processing) => Promise<unknown>;
}) {
  const live = useLiveProcessing({
    processing,
    revision,
    disabled,
    save,
    tryProcessing,
  });
  const headingId = useId();
  const profilesId = useId();
  const value = live.value;
  const profiles = value.profiles ?? [];
  const active = profiles.some(({ id }) => id === value.active_profile)
    ? value.active_profile
    : undefined;
  const choices = [
    ...profiles.map((profile) => ({ id: profile.id, label: nameOf(profile) })),
    { id: undefined, label: 'No correction' },
  ];
  return (
    <section className="quick-picture" aria-labelledby={headingId}>
      <h3 id={headingId} className="visually-hidden">
        Picture
      </h3>
      <FieldScopeProvider value={live.scope}>
        <div key={live.generation}>
          <RangeField
            path="processing.brightness_percent"
            label="Brightness"
            className="brightness"
            value={value.brightness_percent}
            onChange={(brightness_percent) =>
              live.slider.slide({ ...live.value, brightness_percent })
            }
            onPress={live.slider.press}
            onRelease={live.slider.release}
            format={percent}
            {...processingLimits.brightness_percent}
          />
        </div>
      </FieldScopeProvider>
      <div className="profile-chips" role="group" aria-labelledby={profilesId}>
        <span id={profilesId} className="chips-label">
          Colour profile
        </span>
        <div className="chips">
          {choices.map((choice) => (
            <button
              type="button"
              key={choice.id ?? ''}
              className="chip"
              data-profile={choice.id ?? ''}
              aria-pressed={choice.id === active}
              disabled={disabled}
              onClick={() => {
                if (choice.id !== active)
                  void live.commit(
                    withProfiles(live.value, [...profiles], choice.id),
                  );
              }}
            >
              {choice.label}
            </button>
          ))}
          {profiles.length ? null : (
            <button
              type="button"
              className="chip add"
              disabled={disabled}
              onClick={calibrate}
            >
              Calibrate colours
            </button>
          )}
        </div>
      </div>
      <Problems notice={live.notice} failure={live.failure} />
    </section>
  );
}

/**
 * Picture settings apply live: processing-only changes reach a running
 * session at the next frame without restarting lighting, so each change is
 * saved shortly after the user stops adjusting it. The rarely needed
 * controls sit under Advanced.
 */
export function ProcessingPanel({
  processing,
  revision,
  disabled,
  running,
  save,
}: PictureProps & { running: boolean }) {
  const live = useLiveProcessing({ processing, revision, disabled, save });
  const { failure, notice, scope, replace } = live;
  const headingId = useId();
  function change(next: Partial<ProcessingControls>) {
    replace({ ...live.value, ...next });
  }
  const value = live.value;
  let status = running
    ? 'Changes show on your lights right away.'
    : 'Changes are used when screen sync is on.';
  if (live.saving) status = 'Applying…';
  else if (live.dirty) status = failure ? 'Not applied' : 'Waiting to apply…';
  return (
    <section
      className="settings-section processing"
      aria-labelledby={headingId}
    >
      <div className="section-heading">
        <h2 id={headingId}>Picture</h2>
        <span
          className={`save-state${live.dirty ? ' attention' : ''}`}
          role="status"
        >
          {status}
        </span>
      </div>
      <FieldScopeProvider value={scope}>
        <div key={`basic-${live.generation}`}>
          <RangeField
            path="processing.brightness_percent"
            label="Brightness"
            value={value.brightness_percent}
            onChange={(brightness_percent) => change({ brightness_percent })}
            format={percent}
            {...processingLimits.brightness_percent}
          />
          <CheckboxField
            path="processing.remove_black_bars"
            label="Remove black bars"
            help="Ignores letterbox bars when choosing colours."
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
        <div key={`advanced-${live.generation}`}>
          <details className="disclosure advanced">
            <summary>Advanced</summary>
            <div className="disclosure-body">
              <RangeField
                path="processing.shadow_lift_percent"
                label="Shadow lift"
                help="Lifts dark colours; black stays off."
                value={value.shadow_lift_percent}
                onChange={(shadow_lift_percent) =>
                  change({ shadow_lift_percent })
                }
                format={percent}
                {...processingLimits.shadow_lift_percent}
              />
              <RangeField
                path="processing.spatial_reach_percent"
                label="Spatial reach"
                help="How far into the picture each LED looks."
                value={value.spatial_reach_percent}
                onChange={(spatial_reach_percent) =>
                  change({ spatial_reach_percent })
                }
                format={percent}
                {...processingLimits.spatial_reach_percent}
              />
              <div className="grid">
                <NumberField
                  path="processing.smoothing_seconds"
                  label="Smoothing"
                  unit="seconds"
                  help="0 turns smoothing off."
                  value={value.smoothing_seconds}
                  onChange={(smoothing_seconds) =>
                    change({ smoothing_seconds })
                  }
                  {...processingLimits.smoothing_seconds}
                />
                <NumberField
                  path="processing.sample_density"
                  label="Sample density"
                  unit="per 1000 pixels"
                  help="Higher follows detail more closely and uses more CPU. 128 is a good start."
                  value={value.sample_density}
                  onChange={(sample_density) => change({ sample_density })}
                  {...processingLimits.sample_density}
                />
              </div>
            </div>
          </details>
        </div>
      </FieldScopeProvider>
      <Problems notice={notice} failure={failure} />
    </section>
  );
}
