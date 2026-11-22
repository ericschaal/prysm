import { useId, useState } from 'react';
import type { Processing, StyleValues } from '@prysm/contracts/application';
import { FieldScopeProvider } from './form';
import { CheckboxField, RangeField } from './fields';
import {
  processingLimits,
  styleLimits,
  useLiveProcessing,
  withCalibrations,
  type ProcessingControls,
} from './live-processing';
import { RowChoice, RowDisclosure } from './rows';
import { useSaveReport } from './save-state';
import { chooseStyle, styleOrder, styleValues, styles } from './styles';

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
      label: calibration.name,
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
 * The style, on its own page: one row per style, the chosen one ticked,
 * applied at once. Under the list, “Adjust this style” opens the sliders of
 * the ticked style; it is open while Custom is chosen and otherwise closed
 * until opened, so nothing above it moves. Moving a slider of a built-in
 * style makes it Custom, from that style's values with the change. The
 * sliders apply live, tried on the lights while dragged and saved where they
 * are let go, as on Home. Choosing Custom returns to the kept values; a
 * first Custom starts from the style that was ticked.
 */
export function StyleSettings(props: LiveProps) {
  const { live } = useControls(props, 'style');
  const nameId = useId();
  const headingId = useId();
  const value = live.value;
  const chosen = value.style.kind;
  const [open, setOpen] = useState(chosen === 'custom');
  /** What the sliders show: the kept values, or the ticked style's own. */
  const shown = chosen === 'custom' ? value.custom_style : styleValues[chosen];
  /** A slider's props: it saves Custom with `values` and its own change. */
  const knob = <K extends keyof StyleValues>(key: K, values: StyleValues) => {
    const path = `processing.custom_style.${key}`;
    return {
      path,
      value: values[key],
      onChange: (next: number) =>
        live.slider.slide(
          {
            ...live.value,
            style: { kind: 'custom' },
            custom_style: { ...values, [key]: next },
          },
          path,
        ),
      onPress: live.slider.press,
      onRelease: live.slider.release,
    };
  };
  return (
    <FieldScopeProvider value={live.scope}>
      <section className="group" aria-labelledby={headingId}>
        <h2 id={headingId} className="overline">
          Styles
        </h2>
        <ul className="rows" role="radiogroup" aria-labelledby={headingId}>
          {styleOrder.map((style) => (
            <RowChoice
              key={style}
              name={nameId}
              label={styles[style].label}
              about={styles[style].about}
              checked={chosen === style}
              disabled={props.disabled}
              onChoose={() => {
                if (style === 'custom') setOpen(true);
                if (style !== chosen)
                  void live.commit(
                    chooseStyle(live.value, style),
                    'processing.style',
                  );
              }}
            />
          ))}
        </ul>
      </section>
      <RowDisclosure
        heading="Values"
        label="Adjust this style"
        about="Moving a slider switches to Custom."
        open={open}
        onToggle={() => setOpen(!open)}
      >
        {shown ? (
          <div className="sliders" key={live.generation}>
            <RangeField
              label="Smoothing"
              help="Softens quick changes. 0 turns it off."
              format={seconds}
              {...knob('smoothing_seconds', shown)}
              // Steps of 0.05 s, without the slider's binary fractions.
              onChange={(next) =>
                knob('smoothing_seconds', shown).onChange(
                  Math.round(next * 100) / 100,
                )
              }
              {...styleLimits.smoothing_seconds}
            />
            <RangeField
              label="Scene cuts"
              help="How much of a scene cut the lights take at once. 0 eases cuts in like everything else."
              format={percent}
              {...knob('cut_response_percent', shown)}
              {...styleLimits.cut_response_percent}
            />
            <RangeField
              label="Brightness ramp"
              help="The least time from dark to full brightness. 0 follows at once."
              format={seconds}
              {...knob('brightness_ramp_seconds', shown)}
              {...styleLimits.brightness_ramp_seconds}
            />
            <RangeField
              label="Edge depth"
              help="How far into the picture each light looks."
              format={percent}
              {...knob('spatial_reach_percent', shown)}
              {...styleLimits.spatial_reach_percent}
            />
            <RangeField
              label="Colour focus"
              help="Each light keeps the colour next to it and takes less from the rest of the picture. 0 is off."
              format={percent}
              {...knob('color_focus_percent', shown)}
              {...styleLimits.color_focus_percent}
            />
            <RangeField
              label="Colour boost"
              help="Makes the lights’ colours stronger. 0% leaves them as they are."
              format={percent}
              {...knob('saturation_boost_percent', shown)}
              {...styleLimits.saturation_boost_percent}
            />
            <RangeField
              label="Dark-scene glow"
              help="A faint white glow the lights keep while the picture is dark. 0% lets them go fully dark."
              format={percent}
              {...knob('dark_floor_percent', shown)}
              {...styleLimits.dark_floor_percent}
            />
          </div>
        ) : null}
      </RowDisclosure>
    </FieldScopeProvider>
  );
}

/**
 * Detail, on its own page: how closely the lights follow small details,
 * outside the styles. Applies live like the style's sliders.
 */
export function Detail(props: LiveProps) {
  const { live, slider } = useControls(props, 'detail');
  return (
    <FieldScopeProvider value={live.scope}>
      <div className="sliders" key={live.generation}>
        <RangeField
          label="Detail"
          help="Higher follows small details more closely but works the computer harder. 128 suits most screens."
          {...slider('sample_density')}
          {...processingLimits.sample_density}
        />
      </div>
    </FieldScopeProvider>
  );
}
