import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from 'react';
import {
  failureOf,
  plainFailure,
  same,
  type Failure,
  type LightingTarget,
} from '@prysm/application-client';
import {
  adapterFor,
  createSection,
  kindsOf,
  refusal,
  type AdapterSection,
  type AdapterUis,
} from './adapters';
import { useDraft } from './draft';
import {
  FieldScopeProvider,
  focusField,
  saveProblem,
  showsField,
  useFieldErrors,
  useFieldScope,
} from './form';
import { RowLink } from './rows';
import { SectionFields } from './section-step';
import { stepCopy } from './step-copy';
import type { PathState } from './setup-card';

/** How long a new source or lights may take to show they work. */
export const checkForMs = 10_000;
/** Screen sync must keep flowing this long to count as working. */
export const workingAfterMs = 1500;

/** Why a saved section does not work, to follow “Saved, but”. */
function notWorking(section: 'capture' | 'output', failure: Failure | null) {
  if (failure) {
    const plain = plainFailure(failure, 'processed', 'Device effects');
    return plain.charAt(0).toLowerCase() + plain.slice(1);
  }
  return section === 'capture'
    ? 'no picture has arrived.'
    : 'the lights haven’t started.';
}

/**
 * Where the editor stands. A save is followed by a check while screen sync
 * is on; with it off the user may try it now.
 */
type Phase =
  | { kind: 'editing' }
  | { kind: 'saving' }
  | { kind: 'conflict' }
  /** Saved; screen sync on: watching whether it works. */
  | { kind: 'checking'; before: Failure | null }
  /** Saved; screen sync off, so not tried. */
  | { kind: 'untried' }
  | { kind: 'reverted' }
  | { kind: 'problem'; message: string };

export interface SectionEditorProps<S extends AdapterSection> {
  section: 'capture' | 'output';
  adapters: AdapterUis<S>;
  /** The observed section; `null` while unconfigured. */
  saved: S | null;
  revision: number;
  disabled: boolean;
  /** Screen sync now: what the user asked for, the path and its failure. */
  target: LightingTarget;
  path: PathState;
  failure: Failure | null;
  /** Save the section against `expected`; resolves with the new revision. */
  save: (expected: number, value: S) => Promise<number>;
  /** Start screen sync, to try what was saved. */
  startSync: () => Promise<unknown>;
  /**
   * What was saved before the last change made here, kept by the portal so
   * it is still offered after leaving the page; `null` when there is none.
   */
  previous: S | null;
  setPrevious: (previous: S | null) => void;
  /** Back to Settings. */
  leave: () => void;
  /** Open the live preview, when this host has one. */
  showPreview?: () => void;
}

/**
 * One section on its own page: the same step as in setup, then one button
 * that saves it. Saving restarts screen sync, so what follows says whether
 * the new video source or lights work, and offers the previous one back.
 */
export function SectionEditor<S extends AdapterSection>({
  section,
  adapters,
  saved,
  revision,
  disabled,
  target,
  path,
  failure,
  save,
  startSync,
  previous,
  setPrevious,
  leave,
  showPreview,
}: SectionEditorProps<S>) {
  const copy = stepCopy[section];
  const draft = useDraft<S>({
    saved,
    revision,
    fresh: () => createSection(adapters, kindsOf(adapters)[0] ?? ''),
  });
  const errors = useFieldErrors();
  const [phase, setPhase] = useState<Phase>({ kind: 'editing' });
  const [attempted, setAttempted] = useState(false);
  /** The check's timers: flowing long enough, or out of time. */
  const [settled, setSettled] = useState(false);
  const [timedOut, setTimedOut] = useState(false);
  const formRef = useRef<HTMLFormElement>(null);
  const [focus, setFocus] = useState<{ field?: string } | null>(null);
  useEffect(() => {
    if (focus) focusField(formRef.current, focus.field);
  }, [focus]);
  const busy = phase.kind === 'saving' || draft.syncing;
  const locked = disabled || busy;
  const scope = useFieldScope(locked, errors, attempted);
  const adapter = adapterFor(adapters, draft.draft.kind);
  const invalid = Object.keys(errors.local).length > 0;

  // The check, while screen sync is on: a new failure on this side (or in
  // Prysm) means it does not work; flowing for a moment means it does.
  const checking = phase.kind === 'checking' && !draft.syncing;
  const failedHere =
    checking &&
    path.kind === 'failed' &&
    (path.at === section || path.at === null) &&
    failure !== null &&
    !same(failure, phase.before);
  const flowing = checking && path.kind === 'flowing';
  /** This side does not work now (as its row in Settings says). */
  const problemHere =
    path.kind === 'failed' && path.at === section && failure !== null;
  useEffect(() => {
    if (!flowing) return;
    const timer = setTimeout(() => setSettled(true), workingAfterMs);
    return () => clearTimeout(timer);
  }, [flowing]);
  useEffect(() => {
    if (!checking) return;
    const timer = setTimeout(() => setTimedOut(true), checkForMs);
    return () => clearTimeout(timer);
  }, [checking]);
  let outcome: 'none' | 'working' | 'failed' | 'waiting' = 'none';
  if (checking) {
    if (failedHere) outcome = 'failed';
    else if (settled && flowing) outcome = 'working';
    else if (timedOut) outcome = 'failed';
    else outcome = 'waiting';
  }

  function change(next: S) {
    draft.setDraft(next);
    if (phase.kind !== 'editing') setPhase({ kind: 'editing' });
  }

  async function store(value: S, after: Phase['kind'], was = draft.base) {
    setPhase({ kind: 'saving' });
    const before = failure;
    try {
      const next = await save(revision, value);
      draft.setDraft(value);
      draft.saved(value, next);
      setAttempted(false);
      setSettled(false);
      setTimedOut(false);
      if (after === 'reverted') {
        setPrevious(null);
        setPhase({ kind: 'reverted' });
        return;
      }
      setPrevious(was);
      setPhase(
        target === 'processed'
          ? { kind: 'checking', before }
          : { kind: 'untried' },
      );
    } catch (error) {
      const reason = failureOf(error);
      if (reason.kind === 'stale_revision') {
        setPhase({ kind: 'conflict' });
        return;
      }
      const field = reason.kind === 'invalid' ? reason.field : undefined;
      if (field) errors.setServer({ [field]: refusal(adapter, field) });
      const shown = showsField(formRef.current, field);
      setPhase({
        kind: 'problem',
        message: saveProblem(shown ? reason : { ...reason, field: undefined }),
      });
      setFocus({ field });
    }
  }

  function saveDraft() {
    setAttempted(true);
    if (invalid) {
      setPhase({
        kind: 'problem',
        message: 'Not saved. Check the marked field.',
      });
      setFocus({ field: Object.keys(errors.local)[0] });
      return;
    }
    if (draft.conflicted) {
      setPhase({ kind: 'conflict' });
      return;
    }
    void store(adapter?.normalize?.(draft.draft) ?? draft.draft, 'checking');
  }
  function submit(event: FormEvent) {
    event.preventDefault();
    if (!locked) saveDraft();
  }
  function takeTheirs() {
    draft.reload();
    errors.clearServer();
    setPhase({ kind: 'editing' });
  }
  function keepMine() {
    // Edit on top of what is saved now, then save as usual.
    draft.rebase(draft.draft);
    void store(
      adapter?.normalize?.(draft.draft) ?? draft.draft,
      'checking',
      saved,
    );
  }
  async function tryNow() {
    setSettled(false);
    setTimedOut(false);
    setPhase({ kind: 'checking', before: null });
    try {
      await startSync();
    } catch {
      // The failure shows in the state; the check reports it.
    }
  }

  // One line, in one place, saying where things stand.
  const thing = section === 'capture' ? 'video source' : 'lights';
  let line: ReactNode = null;
  let tone: 'error' | 'warning' | 'ok' | undefined;
  if (phase.kind === 'saving' || busy) line = 'Saving…';
  else if (phase.kind === 'conflict') {
    line = `The ${thing} ${section === 'capture' ? 'was' : 'were'} changed on another device since you opened this.`;
    tone = 'warning';
  } else if (phase.kind === 'problem') {
    line = phase.message;
    tone = 'error';
  } else if (phase.kind === 'untried') {
    line = `Saved. Screen sync is off, so ${section === 'capture' ? 'this source hasn’t' : 'these lights haven’t'} been tried.`;
  } else if (phase.kind === 'reverted') {
    line = `Saved. Back to ${copy.previous}.`;
    tone = 'ok';
  } else if (outcome === 'waiting')
    line =
      section === 'capture'
        ? 'Saved. Waiting for the picture…'
        : 'Saved. Starting the lights…';
  else if (outcome === 'working') {
    line =
      section === 'capture'
        ? 'Saved. The picture is arriving.'
        : 'Saved. The lights are working.';
    tone = 'ok';
  } else if (outcome === 'failed') {
    line = `Saved, but ${notWorking(section, failure)}`;
    tone = 'error';
  } else if (phase.kind === 'editing' && problemHere) {
    // Opened while this side does not work: say so, as its row does.
    line = notWorking(section, failure).replace(/^\w/, (c) => c.toUpperCase());
    tone = 'error';
  }

  const goBack =
    previous !== null &&
    (outcome === 'failed' ||
      outcome === 'waiting' ||
      phase.kind === 'untried' ||
      (phase.kind === 'editing' && problemHere));
  let actions: ReactNode;
  if (phase.kind === 'conflict')
    actions = (
      <>
        <button type="button" disabled={locked} onClick={takeTheirs}>
          Use theirs
        </button>
        <button
          type="button"
          className="primary"
          disabled={locked}
          onClick={keepMine}
        >
          Keep mine
        </button>
      </>
    );
  else if (draft.dirty || draft.unconfigured || phase.kind === 'problem')
    actions = (
      <>
        <button type="button" disabled={busy} onClick={leave}>
          Cancel
        </button>
        <button className="primary" type="submit" disabled={locked}>
          {busy ? 'Saving…' : copy.use}
        </button>
      </>
    );
  else
    actions = (
      <>
        {goBack ? (
          <button
            type="button"
            disabled={locked}
            onClick={() => previous && void store(previous, 'reverted')}
          >
            Go back to {copy.previous}
          </button>
        ) : null}
        {phase.kind === 'untried' ? (
          <button
            type="button"
            className="primary"
            disabled={locked}
            onClick={() => void tryNow()}
          >
            Turn on screen sync to try
          </button>
        ) : (
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={leave}
          >
            {goBack ? copy.keep : 'Done'}
          </button>
        )}
      </>
    );

  return (
    <form
      ref={formRef}
      className="section-editor"
      aria-label={copy.title}
      aria-busy={busy}
      noValidate
      onSubmit={submit}
    >
      <p className="intro">
        {copy.intro} Saving restarts screen sync if it is on.
      </p>
      {showPreview ? (
        <ul className="rows editor-preview">
          <RowLink
            label={
              section === 'capture' ? 'See the picture' : 'Check placement'
            }
            value={
              section === 'capture'
                ? 'What Prysm captures, live'
                : 'The colours sent to each edge, around the picture'
            }
            onClick={showPreview}
          />
        </ul>
      ) : null}
      <FieldScopeProvider value={scope}>
        <SectionFields
          key={draft.generation}
          section={section}
          adapters={adapters}
          value={draft.draft}
          base={draft.base}
          onChange={change}
        />
      </FieldScopeProvider>
      <p
        className="form-status"
        data-tone={tone}
        role="status"
        tabIndex={-1}
        data-form-alert
      >
        {line}
      </p>
      <div className="actions">{actions}</div>
    </form>
  );
}
