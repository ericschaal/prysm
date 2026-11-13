import type {
  ColorProfile,
  CorrectionControls,
  HueControls,
} from '@prysm/contracts/application';

/**
 * From the generated schemas: `Processing.profiles` `maxItems`
 * (`MAX_PROFILES`) and `ColorProfile.name` `maxLength` (characters).
 */
export const profileLimits = { count: 32, name: 64 } as const;

const control = { min: -100, max: 100, step: 1 } as const;
/** Each correction control, from the generated `CorrectionControls` schema. */
export const correctionLimits = {
  warmth: control,
  tint: control,
  midtones: control,
  intensity: control,
} as const;
/** Each hue anchor, from the generated `HueControls` schema. */
export const hueLimits = {
  red: control,
  yellow: control,
  green: control,
  cyan: control,
  blue: control,
  magenta: control,
} as const;

const noHue: HueControls = {
  red: 0,
  yellow: 0,
  green: 0,
  cyan: 0,
  blue: 0,
  magenta: 0,
};
export const noCorrection: CorrectionControls = {
  warmth: 0,
  tint: 0,
  midtones: 0,
  intensity: 0,
  hue: noHue,
};

/** A profile's correction with absent controls as 0, as the engine reads it. */
export function correctionOf(profile: ColorProfile): CorrectionControls {
  const correction: Partial<CorrectionControls> = profile.correction ?? {};
  return {
    ...noCorrection,
    ...correction,
    hue: { ...noHue, ...correction.hue },
  };
}

/**
 * A new random id (UUID v4). `crypto.randomUUID` needs a secure context,
 * which a portal on the LAN over plain HTTP is not; `getRandomValues` is not
 * restricted.
 */
export function newProfileId(taken: readonly string[] = []): string {
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

/** `base`, else `base 2`, `base 3`, … — the first no profile uses, within the length limit. */
export function freeName(base: string, profiles: readonly ColorProfile[]) {
  const names = new Set(profiles.map((profile) => profile.name));
  for (let count = 1; ; count++) {
    const suffix = count === 1 ? '' : ` ${count}`;
    const name =
      [...base]
        .slice(0, profileLimits.name - suffix.length)
        .join('')
        .trim() + suffix;
    if (!names.has(name)) return name;
  }
}

/** Names are required, but a list edited by hand may lack one. */
export const nameOf = (profile: ColorProfile): string =>
  profile.name ?? profile.id;

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
  [
    'intensity',
    'Intensity',
    'Positive makes colours more vivid, negative makes them paler.',
  ],
] as const;
export const hues = [
  ['red', 'Red'],
  ['yellow', 'Yellow'],
  ['green', 'Green'],
  ['cyan', 'Cyan'],
  ['blue', 'Blue'],
  ['magenta', 'Magenta'],
] as const;
