import { createContext, use, useEffect } from 'react';

/**
 * Where one part of a page stands: nothing pending, edits not saved yet
 * (only a form with its own Save), a save on its way, or a save that failed.
 */
export type SaveState = 'idle' | 'changed' | 'saving' | 'failed';

/** What the page says about its saves. */
export type SavePhase = 'all-saved' | 'saved' | 'changed' | 'saving' | 'failed';

/** How long “Saved” shows before the page says all changes are saved. */
export const savedForMs = 2000;

export const SaveContext = createContext<{
  report: (source: string, state: SaveState) => void;
  phase: SavePhase;
} | null>(null);

/** Tell the page where `source` stands; nothing without a provider. */
export function useSaveReport(source: string, state: SaveState) {
  const report = use(SaveContext)?.report;
  useEffect(() => {
    report?.(source, state);
  }, [report, source, state]);
  useEffect(() => () => report?.(source, 'idle'), [report, source]);
}
