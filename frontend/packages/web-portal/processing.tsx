import {
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import type { Style, StyleValues } from '@prysm/contracts/application';
import { FieldScopeProvider } from './form';
import { CheckboxField, RangeField } from './fields';
import { arrowChoice } from './radio-group';
import {
  processingLimits,
  styleLimits,
  type ProcessingEditor,
  withCalibrations,
  useProcessingEditor,
} from './live-processing';
import { RowChoice, RowDisclosure } from './rows';
import {
  chooseStyle,
  styleOf,
  styleOrder,
  styleValues,
  styles,
} from './styles';

const percent = (value: number) => `${value}%`;
const seconds = (value: number) => `${value}\u00a0s`;

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
function applyStyle(live: ProcessingEditor, style: Style) {
  if (style !== live.value.style.kind)
    void live.commit(chooseStyle(live.value, style), 'processing.style');
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
function SegmentControl<T extends string>({
  labelledBy,
  label,
  action,
  onAction,
  options,
  chosen,
  disabled,
  names = false,
  onChoose,
}: {
  labelledBy: string;
  label: string;
  action: ReactNode;
  onAction: () => void;
  options: readonly { key: T; label: string; about?: string }[];
  chosen: T | null;
  disabled: boolean;
  names?: boolean;
  onChoose: (key: T) => void;
}) {
  const trackRef = useRef<HTMLDivElement>(null);
  const keys = options.map((option) => option.key);
  const index = chosen === null ? -1 : keys.indexOf(chosen);
  // What the row holds, so it is measured again when that changes.
  const labels = options.map((option) => option.label).join('\n');
  // Before the first paint, so the row shows centred with its pill at once.
  useLayoutEffect(() => {
    const track = trackRef.current;
    if (!track) return;
    const chosenItem = () => track.children[index] as HTMLElement | undefined;
    const first = chosenItem();
    if (first)
      track.scrollLeft =
        first.offsetLeft - (track.clientWidth - first.offsetWidth) / 2;
    const mark = () => {
      markMore(track);
      const item = chosenItem();
      if (!item) {
        delete track.dataset.pill;
        return;
      }
      track.style.setProperty('--chosen-x', `${item.offsetLeft}px`);
      track.style.setProperty('--chosen-width', `${item.offsetWidth}px`);
      track.dataset.pill = '';
    };
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
    <div className="card-control">
      <div className="range-heading control-head">
        <span id={labelledBy}>{label}</span>
        <button
          type="button"
          className="link"
          disabled={disabled}
          onClick={onAction}
        >
          {action}
        </button>
      </div>
      <div className="segments" data-names={names || undefined}>
        <div
          ref={trackRef}
          className="segments-scroll"
          role="radiogroup"
          aria-labelledby={labelledBy}
        >
          {options.map((option, at) => (
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
              tabIndex={at === Math.max(0, index) ? 0 : -1}
              disabled={disabled}
              onKeyDown={(event) =>
                arrowChoice(event, keys, option.key, (next) => {
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
    </div>
  );
}

function processingSlider(
  live: ProcessingEditor,
  key: keyof typeof processingLimits,
) {
  const path = `processing.${key}`;
  return {
    path,
    value: live.value[key],
    ...processingLimits[key],
    onPress: live.slider.press,
    onRelease: live.slider.release,
    onChange: (value: number) =>
      live.slider.slide({ ...live.value, [key]: value }, path),
  };
}

/** Brightness, style and active calibration on the light card. */
export function QuickPicture({
  adjustStyle,
  calibrate,
}: {
  /** Opens the Style page. */
  adjustStyle: () => void;
  /** Opens Calibrate. */
  calibrate: () => void;
}) {
  const live = useProcessingEditor();
  const { disabled } = live.scope;
  const headingId = useId();
  const styleId = useId();
  const calibrationsId = useId();
  const value = live.value;
  const style = styleOf(value.style.kind);
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
            label="Brightness"
            className="brightness"
            format={percent}
            {...processingSlider(live, 'brightness_percent')}
          />
        </div>
      </FieldScopeProvider>
      <SegmentControl
        labelledBy={styleId}
        label="Style"
        action={
          <>
            Adjust<span className="visually-hidden"> style</span>
          </>
        }
        onAction={adjustStyle}
        options={offered.map((each) => ({ key: each, ...styles[each] }))}
        chosen={style}
        disabled={disabled}
        onChoose={(next) => applyStyle(live, next)}
      />
      <SegmentControl
        labelledBy={calibrationsId}
        label="Calibration"
        action="Calibrate colours"
        onAction={calibrate}
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
      <Problem failure={live.failure} retry={live.retry} />
    </section>
  );
}

export function BlackBars({ live }: { live: ProcessingEditor }) {
  return (
    <FieldScopeProvider value={live.scope}>
      <div key={live.generation}>
        <CheckboxField
          path="processing.remove_black_bars"
          label="Remove black bars"
          help="Ignores the black bars above and below a film."
          checked={live.value.remove_black_bars}
          onChange={(remove_black_bars) =>
            live.replace(
              { ...live.value, remove_black_bars },
              'processing.remove_black_bars',
            )
          }
        />
      </div>
    </FieldScopeProvider>
  );
}

const styleControls = [
  [
    'smoothing_seconds',
    'Smoothing',
    'Softens quick changes. 0 turns it off.',
    seconds,
  ],
  [
    'cut_response_percent',
    'Scene cuts',
    'How much of a scene cut the lights take at once. 0 eases cuts in like everything else.',
    percent,
  ],
  [
    'brightness_ramp_seconds',
    'Brightness ramp',
    'The least time from dark to full brightness. 0 follows at once.',
    seconds,
  ],
  [
    'spatial_reach_percent',
    'Edge depth',
    'How far into the picture each light looks.',
    percent,
  ],
  [
    'color_focus_percent',
    'Colour focus',
    'Each light keeps the colour next to it and takes less from the rest of the picture. 0 is off.',
    percent,
  ],
  [
    'saturation_boost_percent',
    'Colour boost',
    'Makes the lights’ colours stronger. 0% leaves them as they are.',
    percent,
  ],
  [
    'dark_floor_percent',
    'Dark-scene glow',
    'A faint white glow the lights keep while the picture is dark. 0% lets them go fully dark.',
    percent,
  ],
] as const;

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
export function StyleSettings({ live }: { live: ProcessingEditor }) {
  const nameId = useId();
  const headingId = useId();
  const value = live.value;
  const chosen = styleOf(value.style.kind);
  const [open, setOpen] = useState(chosen === 'custom');
  /** What the sliders show: the kept values, or the ticked style's own. */
  const shown =
    chosen === 'custom' ? value.custom_style : chosen && styleValues[chosen];
  function changeStyle(
    key: keyof StyleValues,
    values: StyleValues,
    next: number,
  ) {
    const updated =
      key === 'smoothing_seconds' ? Math.round(next * 100) / 100 : next;
    live.slider.slide(
      {
        ...live.value,
        style: { kind: 'custom' },
        custom_style: { ...values, [key]: updated },
      },
      `processing.custom_style.${key}`,
    );
  }
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
              disabled={live.scope.disabled}
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
            {styleControls.map(([key, label, help, format]) => (
              <RangeField
                key={key}
                label={label}
                help={help}
                format={format}
                path={`processing.custom_style.${key}`}
                value={shown[key]}
                onChange={(next) => changeStyle(key, shown, next)}
                onPress={live.slider.press}
                onRelease={live.slider.release}
                {...styleLimits[key]}
              />
            ))}
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
export function Detail({ live }: { live: ProcessingEditor }) {
  return (
    <FieldScopeProvider value={live.scope}>
      <div className="sliders" key={live.generation}>
        <RangeField
          label="Detail"
          help="Higher follows small details more closely but works the computer harder. 128 suits most screens."
          {...processingSlider(live, 'sample_density')}
        />
      </div>
    </FieldScopeProvider>
  );
}
