import { useId, useState } from 'react';
import type {
  ColorProfile,
  CorrectionControls,
  HueControls,
} from '@prysm/contracts/application';
import {
  FieldMessage,
  NameField,
  RangeField,
  SelectField,
  useFieldScopeContext,
} from './fields';

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

const noHue: HueControls = { red: 0, yellow: 0, green: 0, cyan: 0, blue: 0, magenta: 0 };
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
  return { ...noCorrection, ...correction, hue: { ...noHue, ...correction.hue } };
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
    const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
    const id = `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
    if (!taken.includes(id)) return id;
  }
}

/** `base`, else `base 2`, `base 3`, … — the first no profile uses, within the length limit. */
export function freeName(base: string, profiles: readonly ColorProfile[]) {
  const names = new Set(profiles.map((profile) => profile.name));
  for (let count = 1; ; count++) {
    const suffix = count === 1 ? '' : ` ${count}`;
    const name = [...base].slice(0, profileLimits.name - suffix.length).join('').trim() + suffix;
    if (!names.has(name)) return name;
  }
}

/** Names are required, but a list edited by hand may lack one. */
export const nameOf = (profile: ColorProfile): string => profile.name ?? profile.id;

export const signed = (value: number) => (value > 0 ? `+${value}` : String(value));

export const corrections = [
  ['warmth', 'Warmth', 'Positive warms the colours, negative cools them.'],
  ['tint', 'Tint', 'Positive adds magenta, negative adds green.'],
  ['midtones', 'Midtones', 'Positive lightens midtones, negative darkens them.'],
  ['intensity', 'Intensity', 'Positive makes colours more vivid, negative makes them paler.'],
] as const;
export const hues = [
  ['red', 'Red'],
  ['yellow', 'Yellow'],
  ['green', 'Green'],
  ['cyan', 'Cyan'],
  ['blue', 'Blue'],
  ['magenta', 'Magenta'],
] as const;

/**
 * Choose, add, rename, duplicate, delete and edit colour profiles. The
 * selected profile is the active one, so its edits show on the lights at
 * once; the Picture panel saves them like its other controls.
 */
export function ProfileControls({
  profiles,
  active,
  onChange,
}: {
  profiles: readonly ColorProfile[];
  active: string | undefined;
  /** The new list and active id; `undefined` applies no correction. */
  onChange: (profiles: ColorProfile[], active: string | undefined) => void;
}) {
  const { disabled, edited } = useFieldScopeContext();
  const [confirming, setConfirming] = useState<string | null>(null);
  const headingId = useId();
  const index = profiles.findIndex((profile) => profile.id === active);
  const selected = index >= 0 ? profiles[index] : undefined;
  const full = profiles.length >= profileLimits.count;
  const ids = profiles.map((profile) => profile.id);

  /** Add, remove or choose a profile: earlier errors on the list no longer apply. */
  function restructure(next: ColorProfile[], id: string | undefined) {
    setConfirming(null);
    edited('processing.profiles');
    edited('processing.active_profile');
    onChange(next, id);
  }
  function add(profile: Omit<ColorProfile, 'id'>) {
    const id = newProfileId(ids);
    restructure([...profiles, { ...profile, id }], id);
  }
  function update(change: Partial<ColorProfile>) {
    if (!selected) return;
    onChange(
      profiles.map((profile, at) => (at === index ? { ...profile, ...change } : profile)),
      active,
    );
  }
  function correct(change: Partial<CorrectionControls>) {
    if (selected) update({ correction: { ...correctionOf(selected), ...change } });
  }
  const path = `processing.profiles[${index}]`;
  const correction = selected ? correctionOf(selected) : noCorrection;
  return (
    <section className="profiles" aria-labelledby={headingId}>
      <h3 id={headingId}>Colour profile</h3>
      <SelectField
        path="processing.active_profile"
        label="Active profile"
        help="Corrects the colours for one source, such as a games console. Applied as you choose it."
        value={selected ? selected.id : ''}
        options={[
          { value: '', label: 'None (no correction)' },
          ...profiles.map((profile) => ({ value: profile.id, label: nameOf(profile) })),
        ]}
        onChange={(id) => restructure([...profiles], id || undefined)}
      />
      <div className="profile-actions">
        <button
          type="button"
          disabled={disabled || full}
          onClick={() =>
            add({ name: freeName('New profile', profiles), correction: noCorrection })
          }
        >
          New profile
        </button>
        {selected ? (
          <>
            <button
              type="button"
              disabled={disabled || full}
              onClick={() =>
                add({
                  name: freeName(`${nameOf(selected)} copy`, profiles),
                  correction: correctionOf(selected),
                })
              }
            >
              Duplicate
            </button>
            <button
              type="button"
              disabled={disabled}
              onClick={() => setConfirming(selected.id)}
            >
              Delete…
            </button>
          </>
        ) : null}
      </div>
      {full ? (
        <p className="help">
          You have the most profiles Prysm keeps ({profileLimits.count}). Delete
          one to add another.
        </p>
      ) : null}
      <FieldMessage path="processing.profiles" touched />
      {selected && confirming === selected.id ? (
        <div className="notice warning profile-confirm" role="alert">
          <p>Delete “{nameOf(selected)}”? This cannot be undone.</p>
          <button
            type="button"
            disabled={disabled}
            onClick={() =>
              restructure(
                profiles.filter((profile) => profile.id !== selected.id),
                undefined,
              )
            }
          >
            Delete profile
          </button>
          <button type="button" onClick={() => setConfirming(null)}>
            Keep
          </button>
        </div>
      ) : null}
      {selected ? (
        <div className="profile-editor" key={selected.id}>
          <NameField
            path={`${path}.name`}
            label="Profile name"
            help="Saved when you leave the field or press Enter."
            value={selected.name ?? ''}
            maxChars={profileLimits.name}
            onChange={(name) => update({ name })}
          />
          <div className="grid">
            {corrections.map(([name, label, help]) => (
              <RangeField
                key={name}
                path={`${path}.correction.${name}`}
                label={label}
                help={help}
                value={correction[name]}
                onChange={(value) => correct({ [name]: value })}
                format={signed}
                {...correctionLimits[name]}
              />
            ))}
          </div>
          <fieldset>
            <legend>Hue shift</legend>
            <p className="help">
              Turns each colour toward the next: red toward yellow, yellow
              toward green, and so on to magenta toward red.
            </p>
            <div className="grid three">
              {hues.map(([name, label]) => (
                <RangeField
                  key={name}
                  path={`${path}.correction.hue.${name}`}
                  label={label}
                  value={correction.hue[name]}
                  onChange={(value) =>
                    correct({ hue: { ...correction.hue, [name]: value } })
                  }
                  format={signed}
                  {...hueLimits[name]}
                />
              ))}
            </div>
          </fieldset>
          <div className="actions">
            <button
              type="button"
              disabled={disabled}
              onClick={() => update({ correction: noCorrection })}
            >
              Reset correction
            </button>
          </div>
        </div>
      ) : null}
    </section>
  );
}
