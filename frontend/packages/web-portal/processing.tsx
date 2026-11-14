import { useId } from 'react';
import type { Processing } from '@prysm/contracts/application';
import { FieldScopeProvider } from './form';
import { CheckboxField, RangeField } from './fields';
import {
  fineLimits,
  processingLimits,
  useLiveProcessing,
  withCalibrations,
  type ProcessingControls,
} from './live-processing';
import { nameOf } from './calibration-model';
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
 * calibrations as one-tap chips. Dragging brightness changes the lights as
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
  /** Opens Calibrate, offered while there are no calibrations. */
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
  const calibrationsId = useId();
  const value = live.value;
  const calibrations = value.calibrations ?? [];
  const active = calibrations.some(({ id }) => id === value.active_calibration)
    ? value.active_calibration
    : undefined;
  const choices = [
    ...calibrations.map((calibration) => ({
      id: calibration.id,
      label: nameOf(calibration),
    })),
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
      <div
        className="calibration-chips"
        role="group"
        aria-labelledby={calibrationsId}
      >
        <span id={calibrationsId} className="chips-label">
          Calibration
        </span>
        <div className="chips">
          {choices.map((choice) => (
            <button
              type="button"
              key={choice.id ?? ''}
              className="chip"
              data-calibration={choice.id ?? ''}
              aria-pressed={choice.id === active}
              disabled={disabled}
              onClick={() => {
                if (choice.id !== active)
                  void live.commit(
                    withCalibrations(live.value, [...calibrations], choice.id),
                    'processing.active_calibration',
                  );
              }}
            >
              {choice.label}
            </button>
          ))}
          {calibrations.length ? null : (
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

/**
 * Picture fine-tuning, on its own page: the rarely needed controls. They
 * apply live; a slider is tried on the lights while dragged and saved where
 * it is let go, as on Home.
 */
export function FineTuning(props: LiveProps) {
  const { live, slider } = useControls(props, 'fine-tuning');
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
          label="Colour focus"
          help="Each light keeps the colour next to it and takes less from the rest of the picture. 0 is off."
          format={percent}
          {...slider('color_focus_percent')}
          {...processingLimits.color_focus_percent}
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
          label="Scene cuts"
          help="How much of a scene cut the lights take at once. 0 eases cuts in like everything else."
          format={percent}
          {...slider('cut_response_percent')}
          {...processingLimits.cut_response_percent}
        />
        <RangeField
          label="Brightness ramp"
          help="The least time from dark to full brightness. 0 follows at once."
          format={seconds}
          {...slider('brightness_ramp_seconds')}
          {...processingLimits.brightness_ramp_seconds}
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
