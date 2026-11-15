import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from 'react';
import { failureOf, type PreviewClient } from '@prysm/application-client';
import {
  adapterFor,
  createSection,
  kindsOf,
  ledCount,
  refusal,
  type AdapterSection,
  type AdapterUis,
} from './adapters';
import { useDraft } from './draft';
import {
  FieldScopeProvider,
  firstField,
  focusField,
  saveProblem,
  showsField,
  useFieldErrors,
  useFieldScope,
} from './form';
import { useLingering } from './lingering';
import { SectionFields } from './section-step';
import { sectionStatus, type LiveState } from './section-state';
import { SectionStatus } from './section-status';
import { stepCopy } from './step-copy';

/** How long a source may take to send its first picture after a start. */
export const checkForMs = 10_000;

/** How long the save bar says a save works before it goes. */
export const outcomeForMs = 2500;

/** Where the editor stands; whether the section works is the status's. */
type Phase =
  | { kind: 'editing' }
  | { kind: 'saving' }
  | { kind: 'conflict' }
  | { kind: 'problem'; message: string };

/** The saved section in a few words: `WLED at wled.local · 120 LEDs`. */
function identityOf<S extends AdapterSection>(
  adapters: AdapterUis<S>,
  saved: S | null,
) {
  if (!saved) return null;
  const adapter = adapterFor(adapters, saved.kind);
  const label = adapter?.label ?? saved.kind;
  const address = adapter
    ?.facts?.(saved)
    .find((fact) => fact.label === 'Address')?.value;
  const edges = adapter?.edges?.(saved);
  return [
    address ? `${label} at ${address}` : label,
    ...(edges ? [`${ledCount(edges)}\u00a0LEDs`] : []),
  ].join(' · ');
}

/** Prysm's words as a sentence: ended with a full stop if they are not. */
const sentence = (words: string) =>
  /[.!?…]$/.test(words) ? words : `${words}.`;

export interface SectionEditorProps<S extends AdapterSection> {
  section: 'capture' | 'output';
  adapters: AdapterUis<S>;
  /** The observed section; `null` while unconfigured. */
  saved: S | null;
  revision: number;
  disabled: boolean;
  /** Screen sync now, for the status at the top. */
  live: LiveState;
  /** Save the section against `expected`; resolves with the new revision. */
  save: (expected: number, value: S) => Promise<number>;
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
  /** The live picture for the video source's status, when this host has one. */
  preview?: PreviewClient;
}

/**
 * One section on its own page: whether it works now, the same step as in
 * setup, and a bar at the bottom while there is something to save. After a
 * save the bar stays where the user is and follows the status: starting,
 * then working (and goes), or why it fails with the previous one offered.
 */
export function SectionEditor<S extends AdapterSection>({
  section,
  adapters,
  saved,
  revision,
  disabled,
  live,
  save,
  previous,
  setPrevious,
  leave,
  showPreview,
  preview,
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
  /** Saved here: the bar follows the status until it works. */
  const [watching, setWatching] = useState(false);
  const formRef = useRef<HTMLFormElement>(null);
  const [focus, setFocus] = useState<{ field?: string } | null>(null);
  useEffect(() => {
    if (focus) focusField(formRef.current, focus.field);
  }, [focus]);
  const busy = phase.kind === 'saving' || draft.syncing;
  const locked = disabled || busy;
  const scope = useFieldScope(locked, errors, attempted);
  const adapter = adapterFor(adapters, draft.draft.kind);
  const savedUi = saved ? adapterFor(adapters, saved.kind) : undefined;
  /** Fields to fix: their own checks block a save; Prysm's refusals not. */
  const invalid = [
    ...new Set([...Object.keys(errors.local), ...Object.keys(errors.server)]),
  ];
  const blocked = Object.keys(errors.local).length > 0;

  const stalled = useLingering(
    live.target === 'processed' && live.path.kind === 'waiting',
    checkForMs,
    0,
  );
  const status = sectionStatus(
    section,
    live,
    identityOf(adapters, saved),
    stalled,
    saved ? savedUi?.lighting?.(saved).effects : '',
  );
  const failing = status.tone === 'problem';
  const hint = failing ? savedUi?.trouble : undefined;

  // Working, or not in use: said for a moment, then the bar goes.
  const settled = status.tone === 'ok' || status.tone === 'idle';
  useEffect(() => {
    if (!watching || !settled) return;
    const timer = setTimeout(() => setWatching(false), outcomeForMs);
    return () => clearTimeout(timer);
  }, [watching, settled]);

  /** The draft tidied for saving, as its kind asks. */
  const normalized = () => adapter?.normalize?.(draft.draft) ?? draft.draft;

  function change(next: S) {
    draft.setDraft(next);
    setWatching(false);
    if (phase.kind !== 'editing') setPhase({ kind: 'editing' });
  }

  /** Save `value`; `was` is offered back afterwards (`null`: nothing). */
  async function store(value: S, was: S | null) {
    setPhase({ kind: 'saving' });
    try {
      const next = await save(revision, value);
      draft.setDraft(value);
      draft.saved(value, next);
      setAttempted(false);
      setPrevious(was);
      setPhase({ kind: 'editing' });
      setWatching(true);
    } catch (error) {
      const reason = failureOf(error);
      if (reason.kind === 'stale_revision') {
        setPhase({ kind: 'conflict' });
        return;
      }
      const field = reason.kind === 'invalid' ? reason.field : undefined;
      if (field) errors.setServer({ [field]: refusal(adapter, field) });
      // A field shown now is said by itself and the bar's “Fix” line.
      if (field && showsField(formRef.current, field))
        setPhase({ kind: 'editing' });
      else
        setPhase({
          kind: 'problem',
          message: saveProblem({ ...reason, field: undefined }),
        });
      setFocus({ field });
    }
  }

  /** To the first field that needs fixing, its problem shown. */
  function fix() {
    setAttempted(true);
    setFocus({ field: firstField(formRef.current, invalid) });
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    if (locked) return;
    if (blocked) fix();
    else if (draft.conflicted) setPhase({ kind: 'conflict' });
    else void store(normalized(), draft.base);
  }
  function takeTheirs() {
    draft.reload();
    errors.clearServer();
    setPhase({ kind: 'editing' });
  }
  function keepMine() {
    // Edit on top of what is saved now, then save as usual.
    draft.rebase(draft.draft);
    void store(normalized(), saved);
  }

  // The bar at the page's end: while there is something to save, and after
  // a save until it works.
  // A number typed out of range changes no draft, but is still to be fixed.
  const editing =
    draft.dirty ||
    draft.unconfigured ||
    invalid.length > 0 ||
    phase.kind !== 'editing';
  const outcome = !editing && (watching || (previous !== null && failing));
  let line: ReactNode = null;
  let tone: 'ok' | 'error' | 'warning' | undefined;
  let actions: ReactNode = null;
  if (phase.kind === 'conflict') {
    line = `The ${section === 'capture' ? 'video source was' : 'lights were'} changed on another device since you opened this.`;
    tone = 'warning';
    actions = (
      <>
        <button
          type="button"
          className="primary"
          disabled={locked}
          onClick={keepMine}
        >
          Keep mine
        </button>
        <button type="button" disabled={locked} onClick={takeTheirs}>
          Use theirs
        </button>
      </>
    );
  } else if (editing) {
    if (phase.kind === 'problem') {
      line = phase.message;
      tone = 'error';
    } else if (invalid.length && (attempted || !draft.unconfigured)) {
      line = (
        <button type="button" className="link" onClick={fix}>
          Fix {invalid.length} {invalid.length === 1 ? 'field' : 'fields'}
        </button>
      );
      tone = 'error';
    } else if (saved && live.target !== 'off')
      line = 'Saving restarts screen sync.';
    actions = (
      <>
        <button className="primary" type="submit" disabled={locked}>
          {busy ? 'Saving…' : copy.use}
        </button>
        <button type="button" disabled={busy} onClick={leave}>
          Cancel
        </button>
      </>
    );
  } else if (outcome && failing) {
    line = (
      <span>
        {status.title}: {sentence(status.details ?? status.detail)}
        {hint ? <span className="form-status-hint">{hint}</span> : null}
      </span>
    );
    tone = 'error';
    actions =
      previous !== null ? (
        <button
          type="button"
          disabled={locked}
          onClick={() => void store(previous, null)}
        >
          Go back to {copy.previous}
        </button>
      ) : null;
  } else if (outcome) {
    line =
      status.tone === 'idle'
        ? `Saved. ${status.detail || status.title}`
        : `Saved. ${status.title}${status.tone === 'busy' ? '' : '.'}`;
    if (status.tone === 'ok') tone = 'ok';
    if (status.tone === 'warning') tone = 'warning';
  }

  // The large preview shows something only while screen sync runs.
  const open =
    showPreview && saved !== null && live.target === 'processed'
      ? {
          label: section === 'capture' ? 'See the picture' : 'Check placement',
          onOpen: showPreview,
        }
      : undefined;
  return (
    <form
      ref={formRef}
      className="section-editor"
      aria-label={copy.title}
      aria-busy={busy}
      noValidate
      onSubmit={submit}
    >
      <SectionStatus
        section={section}
        status={status}
        preview={preview}
        hint={hint}
        brief={outcome && failing}
        open={open}
      />
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
      {/* Kept at the bottom of the screen while it has something to say. */}
      <div className="page-actions" hidden={!line && !actions}>
        <p
          className="form-status"
          data-tone={tone}
          role="status"
          tabIndex={-1}
          data-form-alert
        >
          {line}
        </p>
        {actions ? <div className="actions">{actions}</div> : null}
      </div>
    </form>
  );
}
