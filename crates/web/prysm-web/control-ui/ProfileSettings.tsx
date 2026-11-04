import { useEffect, useId, useRef, useState } from 'react';
import type { SettingsValues } from './api';
import type { DraftErrors } from './settings-draft';

type Calibration = SettingsValues['calibration'];
type Profile = Calibration['profiles'][number];

function ProfileRow({
  profile,
  active,
  deleted,
  disabled,
  error,
  rename,
  toggleDeleted,
}: {
  profile: Profile;
  active: boolean;
  deleted: boolean;
  disabled: boolean;
  error?: string;
  rename: (name: string) => void;
  toggleDeleted: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [originalName, setOriginalName] = useState<string | null>(null);
  const actionsRef = useRef<HTMLButtonElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const restoreFocusRef = useRef(false);
  const id = useId();
  useEffect(() => {
    if (originalName !== null) {
      inputRef.current?.focus();
      inputRef.current?.select();
    } else if (restoreFocusRef.current) actionsRef.current?.focus();
    restoreFocusRef.current = false;
  }, [originalName, deleted]);
  useEffect(() => {
    if (!open) return;
    function dismiss(event: PointerEvent) {
      if (
        event.target instanceof Node &&
        !actionsRef.current?.parentElement?.contains(event.target)
      )
        setOpen(false);
    }
    function escape(event: KeyboardEvent) {
      if (event.key !== 'Escape') return;
      setOpen(false);
      actionsRef.current?.focus();
    }
    document.addEventListener('pointerdown', dismiss);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('pointerdown', dismiss);
      document.removeEventListener('keydown', escape);
    };
  }, [open]);
  function finishRename() {
    restoreFocusRef.current = true;
    setOriginalName(null);
  }
  function toggleRemoval() {
    setOpen(false);
    restoreFocusRef.current = true;
    toggleDeleted();
  }
  let content;
  if (deleted) {
    content = (
      <>
        <div className="profile-identity">
          <s>{profile.name}</s>
          <small>
            {active
              ? 'On save, this profile will be removed and Screen Sync will use No correction.'
              : 'Will be removed on save.'}
          </small>
        </div>
        <button
          ref={actionsRef}
          className="profile-text-action"
          type="button"
          disabled={disabled}
          onClick={toggleRemoval}
        >
          Undo
        </button>
      </>
    );
  } else if (originalName !== null || error) {
    content = (
      <div className="profile-editor setting">
        <label htmlFor={id}>Profile name</label>
        <input
          ref={inputRef}
          id={id}
          type="text"
          name={`calibration.${profile.id}.name`}
          value={profile.name}
          disabled={disabled}
          aria-invalid={!!error}
          aria-describedby={error ? id + '-error' : undefined}
          onChange={(e) => rename(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              if (!error) finishRename();
            }
            if (e.key === 'Escape' && originalName !== null) {
              e.preventDefault();
              rename(originalName);
              finishRename();
            }
          }}
        />
        <button
          className="profile-text-action"
          type="button"
          disabled={disabled || !!error}
          onClick={finishRename}
        >
          Done
        </button>
        {error && (
          <small id={id + '-error'} className="error">
            {error}
          </small>
        )}
      </div>
    );
  } else {
    content = (
      <>
        <div className="profile-identity">
          <span>{profile.name}</span>
        </div>
        {active && <span className="profile-active">Active</span>}
        <div
          className="profile-actions"
          role="group"
          onBlur={(e) => {
            // WebKit focuses the page's non-tabbable main during a tap on a menu action.
            if (
              e.relatedTarget instanceof HTMLElement &&
              e.relatedTarget.tabIndex >= 0 &&
              !e.currentTarget.contains(e.relatedTarget)
            )
              setOpen(false);
          }}
        >
          <button
            type="button"
            className="profile-more"
            ref={actionsRef}
            aria-label={'Actions for ' + profile.name}
            aria-expanded={open}
            aria-controls={open ? id : undefined}
            disabled={disabled}
            onClick={(e) => {
              e.currentTarget.focus();
              setOpen(!open);
            }}
          >
            •••
          </button>
          {open && (
            <div id={id} className="profile-action-list">
              <button
                type="button"
                disabled={disabled}
                onClick={() => {
                  setOpen(false);
                  setOriginalName(profile.name);
                }}
              >
                Rename
              </button>
              <button
                className="profile-delete"
                type="button"
                disabled={disabled}
                onClick={toggleRemoval}
              >
                Delete
              </button>
            </div>
          )}
        </div>
      </>
    );
  }
  return <li className="profile-row">{content}</li>;
}

export function ProfileSettings({
  calibration,
  deleted,
  errors,
  disabled,
  rename,
  markDeleted,
}: {
  calibration: Calibration;
  deleted: string[];
  errors: DraftErrors;
  disabled: boolean;
  rename: (id: string, name: string) => void;
  markDeleted: (ids: string[]) => void;
}) {
  return (
    <section className="profile-settings" aria-labelledby="profiles-heading">
      <h3 id="profiles-heading">Color profiles</h3>
      {calibration.profiles.length > 0 ? (
        <>
          <p className="profile-caption">
            Saved for Screen Sync. Choose the active profile in Overview.
          </p>
          <ul className="profile-list">
            {calibration.profiles.map((profile) => (
              <ProfileRow
                key={profile.id}
                profile={profile}
                active={calibration.active === profile.id}
                disabled={disabled}
                deleted={deleted.includes(profile.id)}
                error={errors[`calibration.${profile.id}.name`]}
                rename={(name) => rename(profile.id, name)}
                toggleDeleted={() =>
                  markDeleted(
                    deleted.includes(profile.id)
                      ? deleted.filter((id) => id !== profile.id)
                      : [...deleted, profile.id],
                  )
                }
              />
            ))}
          </ul>
          <p className="profile-footnote">
            Create another profile with Calibrate Colors on your TV.
          </p>
        </>
      ) : (
        <div className="profile-empty">
          <strong>No saved profiles</strong>
          <p>
            On your TV, open Prysm and choose Calibrate Colors to create your
            first profile.
          </p>
        </div>
      )}
    </section>
  );
}
