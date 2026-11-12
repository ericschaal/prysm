import { useState } from 'react';
import { same } from './adapters';

/**
 * A local draft of one settings section.
 *
 * - `base` is the saved section the draft started from; `null` while unconfigured.
 * - While the draft is clean it follows every change the server reports.
 * - After a save the draft waits until the state stream shows the returned
 *   revision, so the next save is not compared against an older state.
 * - `conflicted` is true when the saved section no longer matches `base`:
 *   someone else changed it, and saving would overwrite their change.
 */
export function useDraft<T>({
  saved,
  revision,
  fresh,
}: {
  saved: T | null;
  revision: number;
  /** The draft for an unconfigured section. */
  fresh: () => T;
}) {
  const [base, setBase] = useState(saved);
  const [pristine, setPristine] = useState(() => saved ?? fresh());
  const [draft, setDraftValue] = useState(pristine);
  const [seen, setSeen] = useState(saved);
  const [awaiting, setAwaiting] = useState<number | null>(null);
  /** Changes when the draft is replaced from outside, to remount its inputs. */
  const [generation, setGeneration] = useState(0);
  const dirty = !same(draft, pristine);
  const syncing = awaiting !== null && revision < awaiting;

  function load(value: T | null, remount = false) {
    const next = value ?? fresh();
    setBase(value);
    setPristine(next);
    // Remounting inputs drops focus; only do it when what they show changes.
    if (remount || !same(next, draft)) {
      setDraftValue(next);
      setGeneration((count) => count + 1);
    }
  }
  if (!same(seen, saved)) {
    setSeen(saved);
    // Our save arrived (revisions restart randomly with the process, so the
    // section itself also ends the wait).
    if (syncing && same(saved, base)) setAwaiting(null);
    else if (!dirty && !syncing) load(saved);
  }
  return {
    base,
    draft,
    dirty,
    syncing,
    generation,
    conflicted: !syncing && !same(saved, base),
    unconfigured: base === null,
    setDraft: setDraftValue,
    /** Discard the draft and show the saved section. */
    reload: () => load(saved, true),
    /** `value` was saved as `revision`. */
    saved: (value: T, savedRevision: number) => {
      setBase(value);
      setPristine(value);
      setAwaiting(savedRevision);
    },
  };
}
