import type { components } from './api.generated';

type Correction = components['schemas']['ColorCorrection'];
type WhiteBalance = Pick<Correction, 'warmth' | 'tint'>;

export function SwatchPicker({
  label,
  options,
  selected,
  onSelect,
}: {
  label: string;
  options: readonly { label: string; swatch: string }[];
  selected: number | null;
  onSelect: (index: number) => void;
}) {
  return (
    <div className="calibration-swatches" role="group" aria-label={label}>
      {options.map(({ label, swatch }, index) => (
        <button
          key={label}
          aria-pressed={index === selected}
          onClick={() => onSelect(index)}
        >
          <i style={{ background: swatch }} />
          {label}
        </button>
      ))}
    </div>
  );
}

export function CalibrationSlider({
  field,
  label,
  low,
  high,
  value,
  onChange,
  resetLabel,
}: {
  field: keyof Correction;
  resetLabel?: string;
  label: string;
  low: string;
  high: string;
  value: number;
  onChange: (value: number) => void;
}) {
  return (
    <>
      <label className={`calibration-slider slider-${field}`}>
        <span className="slider-heading">
          <span>{label}</span>
          <output>
            {value > 0 ? '+' : ''}
            {value}
          </output>
        </span>
        <input
          type="range"
          min="-100"
          max="100"
          step="1"
          value={value}
          aria-label={label}
          onChange={(event) => onChange(Number(event.target.value))}
        />
        <span className="slider-endpoints" aria-hidden="true">
          <span>{low}</span>
          <span>{high}</span>
        </span>
      </label>
      {resetLabel && (
        <button className="calibration-text-button" onClick={() => onChange(0)}>
          {resetLabel}
        </button>
      )}
    </>
  );
}

export function WhiteBalanceControls({
  value,
  onChange,
}: {
  value: WhiteBalance;
  onChange: (patch: Partial<WhiteBalance>) => void;
}) {
  return (
    <>
      <CalibrationSlider
        field="warmth"
        label="Temperature"
        low="Cooler"
        high="Warmer"
        value={value.warmth}
        onChange={(warmth) => onChange({ warmth })}
      />
      <CalibrationSlider
        field="tint"
        label="Tint"
        low="Greener"
        high="More magenta"
        value={value.tint}
        onChange={(tint) => onChange({ tint })}
      />
      <button
        className="calibration-text-button"
        onClick={() => onChange({ warmth: 0, tint: 0 })}
      >
        Reset white balance
      </button>
    </>
  );
}
