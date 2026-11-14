import { useId } from 'react';
import type {
  BrightnessSmoothing,
  Processing,
} from '@prysm/contracts/application';
import { FieldScopeProvider } from './form';
import { CheckboxField, RangeField } from './fields';
import {
  fineLimits,
  processingLimits,
  useLiveProcessing,
  withProfiles,
  type ProcessingControls,
} from './live-processing';
import { nameOf } from './profile-model';
import { RowChoice } from './rows';
import { useSaveReport } from './save-state';

const percent = (value: number) => `${value}%`;
const seconds = (value: number) => `${value}\u00a0s`;

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

interface LiveProps extends PictureProps {
  /** Light with a processing section unsaved, while a slider is held. */
  tryProcessing: (processing: Processing) => Promise<unknown>;
}

/** One live control of the processing section, saved like the others. */
function useControls(props: LiveProps, source: string) {
  const live = useLiveProcessing(props);
  useSaveReport(source, live.state);
  return {
    live,
    change: <K extends keyof ProcessingControls>(
      key: K,
      next: ProcessingControls[K],
    ) => {
      live.replace({ ...live.value, [key]: next }, `processing.${key}`);
    },
    /** A slider's props: tried while held, saved on release. */
    slider: <K extends keyof ProcessingControls>(key: K) => ({
      path: `processing.${key}`,
      value: live.value[key] as number,
      onChange: (next: number) =>
        live.slider.slide({ ...live.value, [key]: next }, `processing.${key}`),
      onPress: live.slider.press,
      onRelease: live.slider.release,
    }),
  };
}

/** “Remove black bars”, on Prysm's card in Settings; saved as it is switched. */
export function BlackBars(props: LiveProps) {
  const { live, change } = useControls(props, 'black-bars');
  return (
    <FieldScopeProvider value={live.scope}>
      <div key={live.generation}>
        <CheckboxField
          path="processing.remove_black_bars"
          label="Remove black bars"
          help="Ignores the black bars above and below a film."
          checked={live.value.remove_black_bars}
          onChange={(next) => change('remove_black_bars', next)}
        />
      </div>
    </FieldScopeProvider>
  );
}

/** How brightness follows a change, in the order offered; every one named. */
const brightnessChoices = {
  with_color: {
    label: 'With colour',
    about: 'Brightness eases in with the colour.',
  },
  faster: {
    label: 'Faster than colour',
    about: 'Brightness follows a cut at once; colour eases in.',
  },
  limited: {
    label: 'Limited',
    about: 'A sudden flash brightens gradually.',
  },
} as const satisfies Record<
  BrightnessSmoothing,
  { label: string; about: string }
>;

/** “Brightness follows”: one row each, the chosen one ticked. */
function BrightnessChoice({
  value,
  disabled,
  onChange,
}: {
  value: BrightnessSmoothing;
  disabled: boolean;
  onChange: (next: BrightnessSmoothing) => void;
}) {
  const nameId = useId();
  const legendId = useId();
  return (
    <fieldset
      className="brightness-choice"
      role="radiogroup"
      aria-labelledby={legendId}
    >
      <legend id={legendId}>Brightness follows</legend>
      <ul className="rows">
        {(
          Object.keys(brightnessChoices) as (keyof typeof brightnessChoices)[]
        ).map((key) => (
          <RowChoice
            key={key}
            name={nameId}
            label={brightnessChoices[key].label}
            about={brightnessChoices[key].about}
            checked={value === key}
            disabled={disabled}
            onChoose={() => onChange(key)}
          />
        ))}
      </ul>
    </fieldset>
  );
}

/**
 * Picture fine-tuning, on its own page: the rarely needed controls. They
 * apply live; a slider is tried on the lights while dragged and saved where
 * it is let go, as on Home.
 */
export function FineTuning(props: LiveProps) {
  const { live, slider, change } = useControls(props, 'fine-tuning');
  const value = live.value;
  return (
    <FieldScopeProvider value={live.scope}>
      <div className="fine-tuning" key={live.generation}>
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
        <RangeField
          label="Smoothing"
          help="Softens quick changes. 0 turns it off."
          format={seconds}
          {...slider('smoothing_seconds')}
          // Steps of 0.05 s; a saved value above the usual 2 s still fits.
          onChange={(next) =>
            slider('smoothing_seconds').onChange(Math.round(next * 100) / 100)
          }
          {...fineLimits.smoothing(value.smoothing_seconds)}
        />
        <RangeField
          label="Response"
          help="Lets big changes, such as a cut, through faster than small flicker."
          format={percent}
          {...slider('smoothing_response_percent')}
          {...processingLimits.smoothing_response_percent}
        />
        <BrightnessChoice
          value={value.brightness_smoothing}
          disabled={live.scope.disabled}
          onChange={(next) => change('brightness_smoothing', next)}
        />
        <RangeField
          label="Detail"
          help="Higher follows small details more closely but works the computer harder. 128 suits most screens."
          {...slider('sample_density')}
          {...fineLimits.detail(value.sample_density)}
        />
      </div>
    </FieldScopeProvider>
  );
}
