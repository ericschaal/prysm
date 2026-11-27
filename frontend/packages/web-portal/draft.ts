import { useState } from 'react';
import { same } from '@prysm/application-client';

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
  const [draft, setDraftValue] = useState(() => saved ?? fresh());
  const [seen, setSeen] = useState(saved);
  const [awaiting, setAwaiting] = useState<number | null>(null);
  /** Changes when the draft is replaced from outside, to remount its inputs. */
  const [generation, setGeneration] = useState(0);
  const dirty = !same(draft, base ?? fresh());
  const syncing = awaiting !== null && revision < awaiting;

  function load(next: T, remount = false) {
    setBase(saved);
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
    else if (!dirty && !syncing) load(saved ?? fresh());
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
    reload: () => load(saved ?? fresh(), true),
    /** Start again from the saved section, with `next` as the edit on top. */
    rebase: (next: T) => load(next),
    /** `value` was saved as `revision`. */
    saved: (value: T, savedRevision: number) => {
      setBase(value);
      setAwaiting(savedRevision);
    },
  };
}
