import { useEffect, useState } from 'react';
import { savedForMs, type SaveState } from './live-processing';
import { useChrome, usePortal } from './portal-context';

const phaseTexts = {
  'all-saved': 'All changes saved',
  saved: 'Saved',
  saving: 'Saving…',
  failed: 'Not saved',
};

/** The active editor's save status. Leaving its page clears the notice. */
export function SaveStatus({ state }: { state: SaveState }) {
  const { online } = usePortal();
  const { stopped } = useChrome();
  const [previous, setPrevious] = useState(state);
  const [justSaved, setJustSaved] = useState(false);
  if (previous !== state) {
    setPrevious(state);
    if (previous === 'saving' && state === 'idle') setJustSaved(true);
  }
  useEffect(() => {
    if (!justSaved) return;
    const timer = setTimeout(() => setJustSaved(false), savedForMs);
    return () => clearTimeout(timer);
  }, [justSaved]);
  let phase: keyof typeof phaseTexts = state === 'idle' ? 'all-saved' : state;
  if (state === 'idle' && justSaved) phase = 'saved';
  return (
    <p
      className="save-status"
      data-phase={online ? phase : 'offline'}
      role="status"
    >
      <span className="save-status-mark" aria-hidden="true" />
      {online ? phaseTexts[phase] : (stopped ?? 'Not connected. Reconnecting…')}
    </p>
  );
}
