import { useState } from 'react';
import type { SettingsView } from './api';
import { LedLayout } from './LedLayout';
import {
  changeInputType,
  edges,
  inputLabels,
  inputLimits,
  ledLimits,
  numericProcessing,
  type DraftErrors,
  type SettingsDraft,
  type SettingsField,
} from './settings-draft';

type FieldState = {
  settings: SettingsView;
  errors: DraftErrors;
  attempted: boolean;
  disabled: boolean;
};
type Group<K extends keyof SettingsDraft> = FieldState & {
  value: SettingsDraft[K];
  onChange: (value: SettingsDraft[K]) => void;
};

function Field({
  field,
  label,
  value,
  onChange,
  help,
  placeholder,
  type = 'text',
  min,
  max,
  step,
  choices,
  errors,
  attempted,
  disabled,
}: FieldState & {
  field: SettingsField;
  label: string;
  value: string;
  onChange: (value: string) => void;
  help?: string;
  placeholder?: string;
  type?: 'text' | 'number' | 'url';
  min?: number;
  max?: number;
  step?: number | 'any';
  choices?: readonly (readonly [string, string])[];
}) {
  const [touched, setTouched] = useState(false);
  const error = attempted || touched ? errors[field] : undefined;
  const id = field.replace(/\./g, '-');
  const props = {
    id,
    name: field,
    disabled,
    'aria-invalid': !!error,
    'aria-describedby':
      [help && `${id}-help`, error && `${id}-error`]
        .filter(Boolean)
        .join(' ') || undefined,
    onBlur: () => setTouched(true),
  };
  return (
    <div className="setting">
      <label htmlFor={id}>{label}</label>
      {choices ? (
        <select
          {...props}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        >
          {choices.map(([value, title]) => (
            <option value={value} key={value}>
              {title}
            </option>
          ))}
        </select>
      ) : (
        <input
          {...props}
          placeholder={placeholder}
          type={type}
          min={min}
          max={max}
          step={step}
          value={value}
          autoComplete="off"
          onChange={(event) => onChange(event.target.value)}
        />
      )}
      {help ? <p id={`${id}-help`}>{help}</p> : null}
      {error ? (
        <p id={`${id}-error`} className="error" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}

export function InputFields({ value, onChange, ...state }: Group<'input'>) {
  const limits = inputLimits(value.type);
  return (
    <section aria-labelledby="input-heading">
      <h3 id="input-heading">Capture settings</h3>
      {state.settings.nativeWebos ? (
        <p className="field-note">Video comes directly from this TV.</p>
      ) : (
        <Field
          {...state}
          field="input.type"
          label="Capture from"
          value={value.type}
          choices={Object.entries(inputLabels)}
          onChange={(type) => {
            if (type === 'device' || type === 'file' || type === 'webos')
              onChange(
                changeInputType(value, type, state.settings.settings.input),
              );
          }}
        />
      )}
      {value.type === 'device' && (
        <Field
          {...state}
          field="input.device"
          label="Capture device"
          value={value.device}
          onChange={(device) => onChange({ ...value, device })}
          help="Linux device path or macOS AVFoundation unique ID. In a container, this is the mapped device path."
        />
      )}
      {value.type === 'file' && (
        <Field
          {...state}
          field="input.path"
          label="Video file path"
          value={value.path}
          onChange={(path) => onChange({ ...value, path })}
          help="Path visible to Prysm. File playback requires FFmpeg 9+; the standard Docker image does not include FFmpeg."
        />
      )}
      <div className="setting-grid capture-dimensions">
        <Field
          {...state}
          field="input.width"
          label="Capture width"
          value={value.width}
          onChange={(width) => onChange({ ...value, width })}
          type="number"
          {...limits.width}
        />
        <Field
          {...state}
          field="input.height"
          label="Capture height"
          value={value.height}
          onChange={(height) => onChange({ ...value, height })}
          type="number"
          {...limits.height}
        />
        <Field
          {...state}
          field="input.refresh_rate_hz"
          label="Frame rate (Hz)"
          value={value.refresh_rate_hz}
          onChange={(refresh_rate_hz) =>
            onChange({ ...value, refresh_rate_hz })
          }
          type="number"
          {...limits.refresh_rate_hz}
        />
      </div>
      <p className="field-note">
        {value.type === 'webos'
          ? 'Start with 640 × 360 at 30 fps. webOS requires even dimensions and supports up to 60 fps.'
          : 'Choose a resolution and frame rate supported by your capture source.'}
      </p>
    </section>
  );
}

export function OutputFields({ value, onChange, ...state }: Group<'output'>) {
  return (
    <section aria-labelledby="output-heading">
      <h3 id="output-heading">WLED & LED layout</h3>
      <Field
        {...state}
        field="output.url"
        label="WLED address"
        value={value.url}
        onChange={(url) => onChange({ ...value, url })}
        type="url"
        placeholder="http://192.168.1.20/"
        help="Use the local address you open to control WLED in your browser."
      />
      <p className="field-note">
        Viewed from the front, wiring starts at bottom-left and goes up the left
        edge, across the top, down the right, and along the bottom. Use zero for
        an absent edge.
      </p>
      <div className="led-editor">
        <LedLayout leds={value.leds} />
        {edges.map((edge) => (
          <div className={`led-count led-count-${edge}`} key={edge}>
            <Field
              {...state}
              field={`output.leds.${edge}`}
              label={`${edge[0]!.toUpperCase()}${edge.slice(1)} LEDs`}
              value={value.leds[edge]}
              onChange={(count) =>
                onChange({ ...value, leds: { ...value.leds, [edge]: count } })
              }
              type="number"
              {...ledLimits}
            />
          </div>
        ))}
      </div>
    </section>
  );
}

export function ProcessingFields({
  value,
  onChange,
  frameRate,
  ...state
}: Group<'processing'> & { frameRate: string }) {
  const slowBlackBars = value.remove_black_bars && Number(frameRate) < 5;
  return (
    <section aria-labelledby="processing-heading">
      <h3 id="processing-heading">Picture & color</h3>
      {numericProcessing.map(({ key, ...field }) => (
        <Field
          key={key}
          {...state}
          {...field}
          field={`processing.${key}`}
          value={value[key]}
          onChange={(next) => onChange({ ...value, [key]: next })}
          type="number"
        />
      ))}
      <div className="setting toggle-setting">
        <div>
          <label htmlFor="processing-remove_black_bars">
            Remove black bars
          </label>
          <p id="bars-help">
            Detect the active picture before sampling colors.
          </p>
        </div>
        <input
          id="processing-remove_black_bars"
          type="checkbox"
          role="switch"
          checked={value.remove_black_bars}
          disabled={state.disabled}
          aria-describedby={
            slowBlackBars ? 'bars-help bars-warning' : 'bars-help'
          }
          onChange={(event) =>
            onChange({ ...value, remove_black_bars: event.target.checked })
          }
        />
      </div>
      {slowBlackBars ? (
        <p id="bars-warning" className="notice warning" role="note">
          Black-bar removal needs at least 5 fps. Increase the capture rate or
          turn off black-bar removal.
        </p>
      ) : null}
    </section>
  );
}

export function SourceField({ value, onChange, ...state }: Group<'source'>) {
  return (
    <Field
      {...state}
      field="source"
      label="Color source"
      value={value}
      choices={[
        ['prysm', 'Screen sync'],
        ['wled', 'WLED effects'],
      ]}
      onChange={(source) => {
        if (source === 'prysm' || source === 'wled') onChange(source);
      }}
      help="Changing the selected source does not turn lights on."
    />
  );
}
