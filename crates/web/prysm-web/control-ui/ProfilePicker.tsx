import { useEffect, useId, useRef, useState, type KeyboardEvent } from 'react';
import type { SettingsValues } from './api';

export function ProfilePicker({
  calibration,
  disabled,
  onSelect,
}: {
  calibration: SettingsValues['calibration'];
  disabled: boolean;
  onSelect: (id: string | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const id = useId();
  const active = calibration.active ?? null;
  const name =
    calibration.profiles.find((profile) => profile.id === active)?.name ??
    'No correction';
  const locked = disabled || !calibration.profiles.length;
  if (locked && open) setOpen(false);

  useEffect(() => {
    if (!open) return;
    listRef.current
      ?.querySelector<HTMLButtonElement>('[aria-selected=true]')
      ?.focus();
    function outside(event: PointerEvent) {
      if (
        event.target instanceof Node &&
        !triggerRef.current?.parentElement?.contains(event.target)
      )
        setOpen(false);
    }
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);

  function close() {
    setOpen(false);
    triggerRef.current?.focus({ preventScroll: true });
  }
  function navigate(event: KeyboardEvent<HTMLDivElement>) {
    const options = Array.from(event.currentTarget.querySelectorAll('button'));
    const index = options.indexOf(document.activeElement as HTMLButtonElement);
    let next;
    switch (event.key) {
      case 'Escape':
        event.preventDefault();
        close();
        return;
      case 'ArrowDown':
        next = Math.min(index + 1, options.length - 1);
        break;
      case 'ArrowUp':
        next = Math.max(index - 1, 0);
        break;
      case 'Home':
        next = 0;
        break;
      case 'End':
        next = options.length - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    options[next]?.focus();
  }
  return (
    <div
      className="profile-picker"
      onBlur={(event) => {
        if (
          event.relatedTarget instanceof HTMLElement &&
          event.relatedTarget.tabIndex >= 0 &&
          !event.currentTarget.contains(event.relatedTarget)
        )
          setOpen(false);
      }}
    >
      <button
        ref={triggerRef}
        type="button"
        className="profile-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        disabled={locked}
        onClick={() => setOpen(!open)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') setOpen(false);
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            setOpen(true);
          }
        }}
      >
        <span>Color profile</span>
        <strong>{name}</strong>
        <span className="picker-chevron" aria-hidden="true" />
      </button>
      {open && (
        <div
          ref={listRef}
          id={id}
          className="profile-options"
          role="listbox"
          tabIndex={-1}
          aria-label="Color profile"
          onKeyDown={navigate}
        >
          {[...calibration.profiles, { id: null, name: 'No correction' }].map(
            (profile) => (
              <button
                key={profile.id ?? 'none'}
                type="button"
                role="option"
                tabIndex={-1}
                aria-selected={profile.id === active}
                onClick={() => {
                  close();
                  if (profile.id !== active) onSelect(profile.id);
                }}
              >
                <span>{profile.name}</span>
                {profile.id === active && (
                  <span className="profile-check" aria-hidden="true">
                    ✓
                  </span>
                )}
              </button>
            ),
          )}
        </div>
      )}
    </div>
  );
}
