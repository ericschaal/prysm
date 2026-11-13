import { useEffect, useState } from 'react';
import type {
  ColorProfile,
  CorrectionControls,
  Processing,
} from '@prysm/contracts/application';
import { FieldMessage, NameField, RangeField } from './fields';
import { FieldScopeProvider } from './form';
import { useLiveProcessing, withProfiles } from './live-processing';
import {
  correctionLimits,
  correctionOf,
  corrections,
  hueLimits,
  hues,
  nameOf,
  noCorrection,
  profileLimits,
  signed,
} from './profile-model';
import { useSaveReport } from './save-state';

/** How long a deleted profile can be brought back. */
export const undoForMs = 5000;

interface ProfilesProps {
  processing: Processing;
  revision: number;
  disabled: boolean;
  /** Save the whole processing section against `expected`. */
  save: (expected: number, processing: Processing) => Promise<number>;
  /** Light with a processing section unsaved, while a slider is held. */
  tryProcessing: (processing: Processing) => Promise<unknown>;
  /** Make a new profile with the guided calibration. */
  calibrate: () => void;
}

/** A profile deleted on this page: kept in its place, to undo for a while. */
interface Deleted {
  profile: ColorProfile;
  index: number;
  wasActive: boolean;
  undoable: boolean;
}

/**
 * The colour profiles, each a row: its name (renamed in place), whether it
 * is in use, its fine-tuning and Delete. A deleted profile leaves its row in
 * place with Undo for a few seconds, so nothing below moves. New profiles
 * come from the guided calibration. Everything saves as it changes.
 */
export function ProfilesPage({
  processing,
  revision,
  disabled,
  save,
  tryProcessing,
  calibrate,
}: ProfilesProps) {
  const live = useLiveProcessing({
    processing,
    revision,
    disabled,
    save,
    tryProcessing,
  });
  useSaveReport('profiles', live.state);
  const [deleted, setDeleted] = useState<Record<string, Deleted>>({});
  const profiles = live.value.profiles ?? [];
  const active = live.value.active_profile;
  const full = profiles.length >= profileLimits.count;

  function update(id: string, change: Partial<ColorProfile>, path: string) {
    live.slider.slide(
      withProfiles(
        live.value,
        profiles.map((profile) =>
          profile.id === id ? { ...profile, ...change } : profile,
        ),
        active,
      ),
      path,
    );
  }
  function remove(profile: ColorProfile) {
    const index = profiles.findIndex(({ id }) => id === profile.id);
    setDeleted({
      ...deleted,
      [profile.id]: {
        profile,
        index,
        wasActive: active === profile.id,
        undoable: true,
      },
    });
    void live.commit(
      withProfiles(
        live.value,
        profiles.filter(({ id }) => id !== profile.id),
        active === profile.id ? undefined : active,
      ),
      'processing.profiles',
    );
  }
  function undo(gone: Deleted) {
    setDeleted(
      Object.fromEntries(
        Object.entries(deleted).filter(([id]) => id !== gone.profile.id),
      ),
    );
    const next = [...profiles];
    next.splice(Math.min(gone.index, next.length), 0, gone.profile);
    void live.commit(
      withProfiles(live.value, next, gone.wasActive ? gone.profile.id : active),
      'processing.profiles',
    );
  }
  // Undo is offered for a while; the row then just says it was deleted.
  const undoable = Object.values(deleted).filter((gone) => gone.undoable);
  useEffect(() => {
    if (!undoable.length) return;
    const timer = setTimeout(
      () =>
        setDeleted((current) =>
          Object.fromEntries(
            Object.entries(current).map(([id, gone]) => [
              id,
              { ...gone, undoable: false },
            ]),
          ),
        ),
      undoForMs,
    );
    return () => clearTimeout(timer);
  }, [undoable.length]);

  // The rows: the profiles, with deleted ones back in their places.
  const rows: ({ profile: ColorProfile } | { gone: Deleted })[] = profiles.map(
    (profile) => ({ profile }),
  );
  for (const gone of Object.values(deleted).sort((a, b) => a.index - b.index))
    rows.splice(Math.min(gone.index, rows.length), 0, { gone });

  return (
    <FieldScopeProvider value={live.scope}>
      <p className="page-intro">
        A profile corrects the colours for one source, such as a games console.
        Choose the one in use on Home.
      </p>
      {rows.length ? (
        <ul className="profile-list" key={live.generation}>
          {rows.map((row) =>
            'gone' in row ? (
              <li
                className="profile-row deleted"
                key={`gone-${row.gone.profile.id}`}
              >
                <p role="status">
                  Deleted “{nameOf(row.gone.profile)}”.
                  {row.gone.undoable ? (
                    <>
                      {' '}
                      <button
                        type="button"
                        className="link"
                        disabled={disabled || full}
                        onClick={() => undo(row.gone)}
                      >
                        Undo
                      </button>
                    </>
                  ) : null}
                </p>
              </li>
            ) : (
              <ProfileRow
                key={row.profile.id}
                profile={row.profile}
                path={`processing.profiles[${profiles.indexOf(row.profile)}]`}
                inUse={row.profile.id === active}
                disabled={disabled}
                use={() =>
                  void live.commit(
                    withProfiles(live.value, profiles, row.profile.id),
                    'processing.active_profile',
                  )
                }
                update={(change, path) => update(row.profile.id, change, path)}
                remove={() => remove(row.profile)}
                onPress={live.slider.press}
                onRelease={live.slider.release}
              />
            ),
          )}
        </ul>
      ) : (
        <p className="profiles-empty">
          No profiles yet. A profile matches the lights to your TV for one
          source, such as a games console.
        </p>
      )}
      <FieldMessage path="processing.profiles" touched />
      <div className="profiles-new">
        <button
          type="button"
          className="primary"
          disabled={disabled || full}
          onClick={calibrate}
        >
          New profile (guided)
        </button>
        <p className="help">
          {full
            ? `You have the most profiles Prysm keeps (${profileLimits.count}). Delete one to add another.`
            : 'Shows test patterns on the TV and matches the lights to them, step by step.'}
        </p>
      </div>
    </FieldScopeProvider>
  );
}

/** One profile: its name in place, and its fine-tuning when opened. */
function ProfileRow({
  profile,
  path,
  inUse,
  disabled,
  use,
  update,
  remove,
  onPress,
  onRelease,
}: {
  profile: ColorProfile;
  /** Its place in the settings, for errors: `processing.profiles[2]`. */
  path: string;
  inUse: boolean;
  disabled: boolean;
  use: () => void;
  update: (change: Partial<ColorProfile>, path: string) => void;
  remove: () => void;
  onPress: () => void;
  onRelease: () => void;
}) {
  const correction = correctionOf(profile);
  function correct(change: Partial<CorrectionControls>, at: string) {
    update({ correction: { ...correction, ...change } }, at);
  }
  return (
    <li className="profile-row" data-in-use={inUse}>
      <div className="profile-head">
        <NameField
          className="profile-name"
          path={`${path}.name`}
          label="Name"
          value={profile.name ?? ''}
          maxChars={profileLimits.name}
          onChange={(name) => update({ name }, `${path}.name`)}
        />
        <span className="profile-tag">{inUse ? 'In use' : ''}</span>
        <button
          type="button"
          className="danger quiet"
          disabled={disabled}
          onClick={remove}
        >
          Delete
        </button>
      </div>
      <details className="disclosure profile-tune">
        <summary>Fine-tune</summary>
        <div className="disclosure-body">
          <p className="help profile-use">
            {inUse ? (
              'In use: changes show on the lights as you make them.'
            ) : (
              <>
                Not in use, so changes don’t show on the lights.{' '}
                <button
                  type="button"
                  className="link"
                  disabled={disabled}
                  onClick={use}
                >
                  Use it now
                </button>
              </>
            )}
          </p>
          <div className="grid">
            {corrections.map(([name, label, help]) => (
              <RangeField
                key={name}
                path={`${path}.correction.${name}`}
                label={label}
                help={help}
                value={correction[name]}
                onChange={(value) =>
                  correct({ [name]: value }, `${path}.correction.${name}`)
                }
                onPress={onPress}
                onRelease={onRelease}
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
                    correct(
                      { hue: { ...correction.hue, [name]: value } },
                      `${path}.correction.hue.${name}`,
                    )
                  }
                  onPress={onPress}
                  onRelease={onRelease}
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
              onClick={() =>
                update({ correction: noCorrection }, `${path}.correction`)
              }
            >
              Reset to no correction
            </button>
          </div>
        </div>
      </details>
    </li>
  );
}
