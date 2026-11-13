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
import { useSaveReport } from './save-state';

const percent = (value: number) => `${value}%`;

interface PictureProps {
  processing: Processing;
  revision: number;
  disabled: boolean;
  /** Save the whole processing section against `expected`. */
  save: (expected: number, processing: Processing) => Promise<number>;
}

/** Why the last change was not saved, in a slot kept under the controls. */
function Problem({ failure, retry }: { failure: string; retry?: () => void }) {
  return (
    <p className="problem-line" role="status">
      {failure && retry ? (
        <>
          Not saved.{' '}
          <button type="button" className="link" onClick={retry}>
            Try again
          </button>
        </>
      ) : (
        failure
      )}
    </p>
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
              live.slider.slide(
                { ...live.value, brightness_percent },
                'processing.brightness_percent',
              )
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
                    'processing.active_profile',
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
      <Problem failure={live.failure} retry={live.retry} />
    </section>
  );
}

/**
 * Picture settings apply live: processing-only changes reach a running
 * session at the next frame without restarting lighting. A slider is tried on
 * the lights while it is dragged and saved where it is let go, as on Home;
 * other controls save shortly after the last change. The rarely needed
 * controls sit under Fine-tuning.
 */
export function ProcessingPanel({
  processing,
  revision,
  disabled,
  save,
  tryProcessing,
}: PictureProps & {
  /** Light with a processing section unsaved, while a slider is held. */
  tryProcessing: (processing: Processing) => Promise<unknown>;
}) {
  const live = useLiveProcessing({
    processing,
    revision,
    disabled,
    save,
    tryProcessing,
  });
  useSaveReport('picture', live.state);
  const headingId = useId();
  const value = live.value;
  function change<K extends keyof ProcessingControls>(
    key: K,
    next: ProcessingControls[K],
  ) {
    live.replace({ ...live.value, [key]: next }, `processing.${key}`);
  }
  /** A slider's props: tried while held, saved on release. */
  function slider<K extends keyof ProcessingControls>(key: K) {
    return {
      path: `processing.${key}`,
      value: value[key] as number,
      onChange: (next: number) =>
        live.slider.slide({ ...live.value, [key]: next }, `processing.${key}`),
      onPress: live.slider.press,
      onRelease: live.slider.release,
    };
  }
  return (
    <section
      className="settings-section processing"
      aria-labelledby={headingId}
    >
      <h2 id={headingId}>Picture</h2>
      <FieldScopeProvider value={live.scope}>
        <div key={`basic-${live.generation}`}>
          <RangeField
            label="Brightness"
            format={percent}
            {...slider('brightness_percent')}
            {...processingLimits.brightness_percent}
          />
          <CheckboxField
            path="processing.remove_black_bars"
            label="Remove black bars"
            help="Ignores the black bars above and below a film."
            checked={value.remove_black_bars}
            onChange={(next) => change('remove_black_bars', next)}
          />
        </div>
        <ProfileControls
          key={`profiles-${live.generation}`}
          profiles={value.profiles ?? []}
          active={value.active_profile}
          onChange={(profiles, active, path) =>
            live.slider.slide(withProfiles(live.value, profiles, active), path)
          }
          onPress={live.slider.press}
          onRelease={live.slider.release}
        />
        <div key={`advanced-${live.generation}`}>
          <details className="disclosure advanced">
            <summary>Fine-tuning</summary>
            <div className="disclosure-body">
              <RangeField
                label="Shadow lift"
                help="Lifts dark colours; black stays off."
                format={percent}
                {...slider('shadow_lift_percent')}
                {...processingLimits.shadow_lift_percent}
              />
              <RangeField
                label="Edge depth"
                help="How far into the picture each light looks."
                format={percent}
                {...slider('spatial_reach_percent')}
                {...processingLimits.spatial_reach_percent}
              />
              <div className="grid">
                <NumberField
                  path="processing.smoothing_seconds"
                  label="Smoothing"
                  unit="seconds"
                  help="Softens quick changes. 0 turns it off."
                  value={value.smoothing_seconds}
                  onChange={(next) => change('smoothing_seconds', next)}
                  {...processingLimits.smoothing_seconds}
                />
                <NumberField
                  path="processing.sample_density"
                  label="Detail"
                  help="Higher follows small details more closely but works the computer harder. 128 suits most screens."
                  value={value.sample_density}
                  onChange={(next) => change('sample_density', next)}
                  {...processingLimits.sample_density}
                />
              </div>
            </div>
          </details>
        </div>
      </FieldScopeProvider>
    </section>
  );
}
