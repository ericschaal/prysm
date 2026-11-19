import type {
  Calibration,
  CorrectionControls,
  PrimaryControls,
} from '@prysm/contracts/application';

/**
 * From the generated schemas: `Processing.calibrations` `maxItems`
 * (`MAX_CALIBRATIONS`) and `Calibration.name` `maxLength` (characters).
 */
export const calibrationLimits = { count: 32, name: 64 } as const;

const control = { min: -100, max: 100, step: 1 } as const;
/** Each white control, from the generated `CorrectionControls` schema. */
export const correctionLimits = {
  warmth: control,
  tint: control,
  midtones: control,
} as const;
/** Each primary's controls, from the generated `PrimaryControls` schema. */
export const primaryLimits = { hue: control, strength: control } as const;

export type Primary = 'red' | 'green' | 'blue';

const unchanged: PrimaryControls = { hue: 0, strength: 0 };
export const noCorrection: CorrectionControls = {
  warmth: 0,
  tint: 0,
  midtones: 0,
  red: unchanged,
  green: unchanged,
  blue: unchanged,
};

/**
 * A new random id (UUID v4). `crypto.randomUUID` needs a secure context,
 * which a portal on the LAN over plain HTTP is not; `getRandomValues` is not
 * restricted.
 */
export function newCalibrationId(taken: readonly string[] = []): string {
  for (;;) {
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    bytes[6] = (bytes[6]! & 0x0f) | 0x40;
    bytes[8] = (bytes[8]! & 0x3f) | 0x80;
    const hex = Array.from(bytes, (byte) =>
      byte.toString(16).padStart(2, '0'),
    ).join('');
    const id = `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
    if (!taken.includes(id)) return id;
  }
}

/** `base`, else `base 2`, `base 3`, … — the first no calibration uses, within the length limit. */
export function freeName(base: string, calibrations: readonly Calibration[]) {
  const names = new Set(calibrations.map((calibration) => calibration.name));
  for (let count = 1; ; count++) {
    const suffix = count === 1 ? '' : ` ${count}`;
    const name =
      [...base]
        .slice(0, calibrationLimits.name - suffix.length)
        .join('')
        .trim() + suffix;
    if (!names.has(name)) return name;
  }
}

export const signed = (value: number) =>
  value > 0 ? `+${value}` : String(value);

export const corrections = [
  ['warmth', 'Warmth', 'Positive warms the colours, negative cools them.'],
  ['tint', 'Tint', 'Positive adds magenta, negative adds green.'],
  [
    'midtones',
    'Midtones',
    'Positive lightens midtones, negative darkens them.',
  ],
] as const;

/**
 * The red, green and blue controls in plain words: each slider is named by
 * what its two ends do. A positive hue mixes in the next primary (red, green,
 * blue, red), so the right end names that neighbour's direction.
 */
export const primaries: readonly {
  name: Primary;
  label: string;
  hue: readonly [string, string];
}[] = [
  { name: 'red', label: 'Red', hue: ['More pink', 'More orange'] },
  { name: 'green', label: 'Green', hue: ['More yellow', 'More turquoise'] },
  { name: 'blue', label: 'Blue', hue: ['More turquoise', 'More purple'] },
];
export const strengthEnds = ['Softer', 'Stronger'] as const;

/** A slider named by its ends, as a screen reader says it: “Softer or stronger”. */
export const endsLabel = ([start, end]: readonly [string, string]) =>
  `${start} or ${end.toLowerCase()}`;
