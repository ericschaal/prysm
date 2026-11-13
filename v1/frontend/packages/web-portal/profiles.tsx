import { useEffect } from 'react';
import type {
  ColorProfile,
  CorrectionControls,
  Processing,
} from '@prysm/contracts/application';
import { CheckboxField, FieldMessage, NameField, RangeField } from './fields';
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
import { Group, RowLink, RowSwitch } from './rows';
import { useSaveReport } from './save-state';

/** How long a deleted profile can be brought back. */
export const undoForMs = 5000;

/** A profile deleted from its page: kept in the list's place, to undo for a while. */
export interface DeletedProfile {
  profile: ColorProfile;
  index: number;
  wasActive: boolean;
  undoable: boolean;
}

interface LiveProps {
  processing: Processing;
  revision: number;
  disabled: boolean;
  /** Save the whole processing section against `expected`. */
  save: (expected: number, processing: Processing) => Promise<number>;
  /** Light with a processing section unsaved, while a slider is held. */
  tryProcessing: (processing: Processing) => Promise<unknown>;
}

/** The processing section saved live, reported to the page's save status. */
function useProfiles(props: LiveProps) {
  const live = useLiveProcessing(props);
  useSaveReport('profiles', live.state);
  return live;
}

/**
 * The colour profiles as rows: each opens its own page; one deleted there
 * keeps its place here with Undo for a few seconds, so nothing moves. A new
 * profile comes from the guided calibration.
 */
export function ProfileList({
  deleted,
  setDeleted,
  open,
  calibrate,
  ...props
}: LiveProps & {
  /** Deleted on this visit, by id; kept by the portal across pages. */
  deleted: Record<string, DeletedProfile>;
  setDeleted: (
    change: (
      deleted: Record<string, DeletedProfile>,
    ) => Record<string, DeletedProfile>,
  ) => void;
  /** Open a profile's page. */
  open: (id: string) => void;
  /** Make a new profile with the guided calibration. */
  calibrate: () => void;
}) {
  const live = useProfiles(props);
  const profiles = live.value.profiles ?? [];
  const active = live.value.active_profile;
  const full = profiles.length >= profileLimits.count;

  function undo(gone: DeletedProfile) {
    setDeleted((current) =>
      Object.fromEntries(
        Object.entries(current).filter(([id]) => id !== gone.profile.id),
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
  }, [undoable.length, setDeleted]);

  // The rows: the profiles, with deleted ones back in their places.
  const rows: ({ profile: ColorProfile } | { gone: DeletedProfile })[] =
    profiles.map((profile) => ({ profile }));
  for (const gone of Object.values(deleted).sort((a, b) => a.index - b.index))
    rows.splice(Math.min(gone.index, rows.length), 0, { gone });

  return (
    <FieldScopeProvider value={live.scope}>
      <p className="page-intro">
        A profile corrects the colours for one source, such as a games console.
        Choose the one in use on Home.
      </p>
      {rows.length ? (
        <Group title="Profiles">
          {rows.map((row) =>
            'gone' in row ? (
              <li className="row row-fact" key={`gone-${row.gone.profile.id}`}>
                <span className="row-text">
                  <span className="row-label">{nameOf(row.gone.profile)}</span>
                  <span className="row-value" role="status">
                    Deleted.{' '}
                    {row.gone.undoable ? (
                      <button
                        type="button"
                        className="link"
                        disabled={props.disabled || full}
                        onClick={() => undo(row.gone)}
                      >
                        Undo
                      </button>
                    ) : null}
                  </span>
                </span>
              </li>
            ) : (
              <RowLink
                key={row.profile.id}
                label={nameOf(row.profile)}
                value={row.profile.id === active ? 'In use' : 'Not in use'}
                onClick={() => open(row.profile.id)}
              />
            ),
          )}
        </Group>
      ) : null}
      <Group title="New">
        <RowLink
          label="New profile (guided)"
          value={
            full
              ? `You have the most Prysm keeps (${profileLimits.count}). Delete one first.`
              : 'Test patterns on the TV, step by step'
          }
          onClick={() => {
            if (!full && !props.disabled) calibrate();
          }}
        />
      </Group>
      <FieldMessage path="processing.profiles" touched />
    </FieldScopeProvider>
  );
}

/**
 * One profile on its own page: its name, whether it is in use, its colour
 * correction and hue shift, and Delete (back to the list, with Undo there).
 */
export function ProfilePage({
  id,
  deleted,
  leave,
  ...props
}: LiveProps & {
  id: string;
  /** It was deleted: the list shows it in its place, with Undo. */
  deleted: (gone: DeletedProfile) => void;
  /** Back to the list. */
  leave: () => void;
}) {
  const live = useProfiles(props);
  const profiles = live.value.profiles ?? [];
  const active = live.value.active_profile;
  const index = profiles.findIndex((profile) => profile.id === id);
  const profile = profiles[index];
  if (!profile)
    return (
      <p className="page-intro">
        This profile is not there any more.{' '}
        <button type="button" className="link" onClick={leave}>
          Back to the profiles
        </button>
      </p>
    );
  const path = `processing.profiles[${index}]`;
  const correction = correctionOf(profile);
  const inUse = active === id;
  function update(change: Partial<ColorProfile>, at: string) {
    live.slider.slide(
      withProfiles(
        live.value,
        profiles.map((item) =>
          item.id === id ? { ...item, ...change } : item,
        ),
        active,
      ),
      at,
    );
  }
  function correct(change: Partial<CorrectionControls>, at: string) {
    update({ correction: { ...correction, ...change } }, at);
  }
  const shown = profile;
  function remove() {
    deleted({ profile: shown, index, wasActive: inUse, undoable: true });
    void live.commit(
      withProfiles(
        live.value,
        profiles.filter((item) => item.id !== id),
        inUse ? undefined : active,
      ),
      'processing.profiles',
    );
    leave();
  }
  const slider = (name: string) => ({
    path: `${path}.correction.${name}`,
    onPress: live.slider.press,
    onRelease: live.slider.release,
    format: signed,
  });
  return (
    <FieldScopeProvider value={live.scope}>
      <div className="profile-page" key={live.generation}>
        <NameField
          path={`${path}.name`}
          label="Name"
          help="Saved when you leave the field or press Enter."
          value={profile.name ?? ''}
          maxChars={profileLimits.name}
          onChange={(name) => update({ name }, `${path}.name`)}
        />
        <ul className="rows">
          <RowSwitch>
            <CheckboxField
              path="processing.active_profile"
              label="In use"
              help="Its correction shows on the lights. Off: no correction."
              checked={inUse}
              onChange={(on) =>
                void live.commit(
                  withProfiles(live.value, profiles, on ? id : undefined),
                  'processing.active_profile',
                )
              }
            />
          </RowSwitch>
        </ul>
        <div className="grid">
          {corrections.map(([name, label, help]) => (
            <RangeField
              key={name}
              label={label}
              help={help}
              value={correction[name]}
              onChange={(value) =>
                correct({ [name]: value }, `${path}.correction.${name}`)
              }
              {...slider(name)}
              {...correctionLimits[name]}
            />
          ))}
        </div>
        <fieldset>
          <legend>Hue shift</legend>
          <p className="help">
            Turns each colour toward the next: red toward yellow, yellow toward
            green, and so on to magenta toward red.
          </p>
          <div className="grid three">
            {hues.map(([name, label]) => (
              <RangeField
                key={name}
                label={label}
                value={correction.hue[name]}
                onChange={(value) =>
                  correct(
                    { hue: { ...correction.hue, [name]: value } },
                    `${path}.correction.hue.${name}`,
                  )
                }
                {...slider(`hue.${name}`)}
                {...hueLimits[name]}
              />
            ))}
          </div>
        </fieldset>
        <div className="actions">
          <button
            type="button"
            disabled={props.disabled}
            onClick={() =>
              update({ correction: noCorrection }, `${path}.correction`)
            }
          >
            Reset to no correction
          </button>
          <button
            type="button"
            className="danger"
            disabled={props.disabled}
            onClick={remove}
          >
            Delete profile
          </button>
        </div>
      </div>
    </FieldScopeProvider>
  );
}
