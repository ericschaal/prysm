import { createContext, use, useEffect } from 'react';

/**
 * Where one part of a page stands: nothing pending, a save on its way, or a
 * save that failed.
 */
export type SaveState = 'idle' | 'saving' | 'failed';

/** What the page says about its saves. */
export type SavePhase = 'all-saved' | 'saved' | 'saving' | 'failed';

/** How long “Saved” shows before the page says all changes are saved. */
export const savedForMs = 2000;

export const SaveContext = createContext<{
  report: (source: string, state: SaveState) => void;
  /** `source` is gone: it no longer counts, and says nothing on leaving. */
  forget: (source: string) => void;
  phase: SavePhase;
} | null>(null);

/** Tell the page where `source` stands; nothing without a provider. */
export function useSaveReport(source: string, state: SaveState) {
  const context = use(SaveContext);
  const report = context?.report;
  const forget = context?.forget;
  useEffect(() => {
    report?.(source, state);
  }, [report, source, state]);
  // Leaving is not saving: a part that goes while its save is on its way
  // must not make the page say "Saved".
  useEffect(() => () => forget?.(source), [forget, source]);
}
