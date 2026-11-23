import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import type {
  Processing,
  Style,
  StyleValues,
} from '@prysm/contracts/application';
import { FieldScopeProvider } from './form';
import { CheckboxField, RangeField } from './fields';
import { arrowChoice } from './radio-group';
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

/** Applies `style` at once; the chosen one again saves nothing. */
function applyStyle(live: ReturnType<typeof useLiveProcessing>, style: Style) {
  if (style !== live.value.style.kind)
    void live.commit(chooseStyle(live.value, style), 'processing.style');
}

/**
 * A light card control's header, as Brightness has: its name (`id`, which
 * names the control) and, on the right, one quiet action.
 */
function ControlHead({
  id,
  label,
  disabled,
  onAction,
  children,
}: {
  id: string;
  label: string;
  disabled: boolean;
  onAction: () => void;
  /** The action's words. */
  children: ReactNode;
}) {
  return (
    // Laid out as a slider's heading (Brightness's): name left, item right.
    <div className="range-heading control-head">
      <span id={id}>{label}</span>
      <button
        type="button"
        className="link"
        disabled={disabled}
        onClick={onAction}
      >
        {children}
      </button>
    </div>
  );
}

/** Where a row of segments can scroll to: `start end`, `end`, or nothing. */
function markMore(track: HTMLElement) {
  const end = track.scrollWidth - track.clientWidth;
  track.dataset.more = [
    track.scrollLeft > 1 ? 'start' : '',
    track.scrollLeft < end - 1 ? 'end' : '',
  ]
    .filter(Boolean)
    .join(' ');
}

/**
 * One choice as a row of segments on a quiet track, applied at once: a
 * radio group, the arrow keys moving the choice. Equal while they fit;
 * where they don't, the row scrolls sideways, the chosen one centred in
 * view, and fades towards each edge with more beyond it. `names`: the
 * options are names of the person's own, each given room and cut with an
 * ellipsis when long. An option's `about` describes it to screen readers.
 */
function Segments<T extends string>({
  labelledBy,
  options,
  chosen,
  disabled,
  names = false,
  onChoose,
}: {
  labelledBy: string;
  options: readonly { key: T; label: string; about?: string }[];
  chosen: T;
  disabled: boolean;
  names?: boolean;
  onChoose: (key: T) => void;
}) {
  const trackRef = useRef<HTMLDivElement>(null);
  const keys = options.map((option) => option.key);
  const index = keys.indexOf(chosen);
  // What the row holds, so it is measured again when that changes.
  const labels = options.map((option) => option.label).join('\n');
  useEffect(() => {
    const track = trackRef.current;
    if (!track) return;
    const item = track.children[index] as HTMLElement | undefined;
    if (item)
      track.scrollLeft =
        item.offsetLeft - (track.clientWidth - item.offsetWidth) / 2;
    const mark = () => markMore(track);
    mark();
    track.addEventListener('scroll', mark, { passive: true });
    const resized =
      typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(mark);
    resized?.observe(track);
    for (const child of track.children) resized?.observe(child);
    return () => {
      track.removeEventListener('scroll', mark);
      resized?.disconnect();
    };
  }, [index, labels]);
  return (
    <>
      <div className="segments" data-names={names || undefined}>
        <div
          ref={trackRef}
          className="segments-scroll"
          role="radiogroup"
          aria-labelledby={labelledBy}
        >
          {options.map((option) => (
            <button
              type="button"
              role="radio"
              key={option.key}
              className="segment"
              data-value={option.key}
              aria-checked={option.key === chosen}
              aria-describedby={
                option.about ? `${labelledBy}-${option.key}` : undefined
              }
              tabIndex={option.key === chosen ? 0 : -1}
              disabled={disabled}
              onKeyDown={(event) =>
                arrowChoice(event, keys, chosen, (next) => {
                  onChoose(next);
                  return true;
                })
              }
              onClick={() => onChoose(option.key)}
            >
              <span>{option.label}</span>
            </button>
          ))}
        </div>
      </div>
      {options.map((option) =>
        option.about ? (
          <span
            key={option.key}
            id={`${labelledBy}-${option.key}`}
            className="visually-hidden"
          >
            {option.about}
          </span>
        ) : null,
      )}
    </>
  );
}

/**
 * The everyday screen sync controls in the light card, one under the other
 * with one header each: brightness, then the style and the calibration as
 * rows of segments. Dragging brightness changes the lights as it moves
 * (unsaved) and saves where it is let go; a segment applies at once.
 */
export function QuickPicture({
  processing,
  revision,
  disabled,
  save,
  adjustStyle,
  calibrate,
  tryProcessing,
}: PictureProps & {
  /** Opens the Style page. */
  adjustStyle: () => void;
  /** Opens Calibrate. */
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
  const styleId = useId();
  const calibrationsId = useId();
  const value = live.value;
  const style = value.style.kind;
  // Custom only once there is one: chosen, or its values kept.
  const offered = styleOrder.filter(
    (each) => each !== 'custom' || style === 'custom' || !!value.custom_style,
  );
  const calibrations = value.calibrations ?? [];
  const active = calibrations.some(({ id }) => id === value.active_calibration)
    ? value.active_calibration
    : undefined;
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
      <div className="card-control">
        <ControlHead
          id={styleId}
          label="Style"
          disabled={disabled}
          onAction={adjustStyle}
        >
          Adjust<span className="visually-hidden"> style</span>
        </ControlHead>
        <Segments
          labelledBy={styleId}
          options={offered.map((each) => ({ key: each, ...styles[each] }))}
          chosen={style}
          disabled={disabled}
          onChoose={(next) => applyStyle(live, next)}
        />
      </div>
      <div className="card-control">
        <ControlHead
          id={calibrationsId}
          label="Calibration"
          disabled={disabled}
          onAction={calibrate}
        >
          Calibrate colours
        </ControlHead>
        <Segments
          labelledBy={calibrationsId}
          options={[
            ...calibrations.map(({ id, name }) => ({ key: id, label: name })),
            { key: '', label: 'No correction' },
          ]}
          chosen={active ?? ''}
          disabled={disabled}
          names
          onChoose={(next) => {
            const id = next || undefined;
            if (id !== active)
              void live.commit(
                withCalibrations(live.value, [...calibrations], id),
                'processing.active_calibration',
              );
          }}
        />
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
                applyStyle(live, style);
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
