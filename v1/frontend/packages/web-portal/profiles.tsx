import { useId, useState } from 'react';
import type {
  ColorProfile,
  CorrectionControls,
} from '@prysm/contracts/application';
import { useFieldScopeContext } from './form';
import { FieldMessage, NameField, RangeField, SelectField } from './fields';
import {
  correctionLimits,
  correctionOf,
  corrections,
  freeName,
  hueLimits,
  hues,
  nameOf,
  newProfileId,
  noCorrection,
  profileLimits,
  signed,
} from './profile-model';

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
      profiles.map((profile, at) =>
        at === index ? { ...profile, ...change } : profile,
      ),
      active,
    );
  }
  function correct(change: Partial<CorrectionControls>) {
    if (selected)
      update({ correction: { ...correctionOf(selected), ...change } });
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
          ...profiles.map((profile) => ({
            value: profile.id,
            label: nameOf(profile),
          })),
        ]}
        onChange={(id) => restructure([...profiles], id || undefined)}
      />
      <div className="profile-actions">
        <button
          type="button"
          disabled={disabled || full}
          onClick={() =>
            add({
              name: freeName('New profile', profiles),
              correction: noCorrection,
            })
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
              className="danger"
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
            className="danger solid"
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
        <details className="disclosure profile-editor" key={selected.id}>
          <summary>Adjust this profile</summary>
          <div className="disclosure-body">
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
        </details>
      ) : null}
    </section>
  );
}
