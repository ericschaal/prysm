import {
  use,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import {
  SaveContext,
  savedForMs,
  type SavePhase,
  type SaveState,
} from './save-state';

const phaseTexts: Record<SavePhase, string> = {
  'all-saved': 'All changes saved',
  saved: 'Saved',
  saving: 'Saving…',
  failed: 'Not saved',
};

/** The page's saves, from every part that reports one. */
export function SaveStatusProvider({ children }: { children: ReactNode }) {
  const [states, setStates] = useState<Record<string, SaveState>>({});
  const statesRef = useRef(states);
  const [justSaved, setJustSaved] = useState(false);
  const report = useCallback((source: string, state: SaveState) => {
    const before = statesRef.current[source] ?? 'idle';
    if (before === state) return;
    // A save that ended without failing: say so for a moment.
    if (before === 'saving' && state === 'idle') setJustSaved(true);
    statesRef.current = { ...statesRef.current, [source]: state };
    setStates(statesRef.current);
  }, []);
  const forget = useCallback((source: string) => {
    if (!(source in statesRef.current)) return;
    const rest = { ...statesRef.current };
    delete rest[source];
    statesRef.current = rest;
    setStates(rest);
  }, []);
  useEffect(() => {
    if (!justSaved) return;
    const timer = setTimeout(() => setJustSaved(false), savedForMs);
    return () => clearTimeout(timer);
  }, [justSaved]);
  const all = Object.values(states);
  let phase: SavePhase = justSaved ? 'saved' : 'all-saved';
  if (all.includes('failed')) phase = 'failed';
  else if (all.includes('saving')) phase = 'saving';
  const value = useMemo(
    () => ({ report, forget, phase }),
    [report, forget, phase],
  );
  return <SaveContext value={value}>{children}</SaveContext>;
}

/**
 * One line, always in the same place, saying whether the page's changes are
 * saved. Its slot is reserved, so nothing moves as it changes.
 */
export function SaveStatus({ online }: { online: boolean }) {
  const phase = use(SaveContext)?.phase ?? 'all-saved';
  // Offline nothing can be saved; what was changed is kept for later.
  return (
    <p
      className="save-status"
      data-phase={online ? phase : 'offline'}
      role="status"
    >
      <span className="save-status-mark" aria-hidden="true" />
      {online ? phaseTexts[phase] : 'Not connected. Reconnecting…'}
    </p>
  );
}
