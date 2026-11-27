import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from 'react';
import { failureOf, type AdapterSide } from '@prysm/application-client';
import {
  adapterFor,
  createSection,
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
import { useNav, usePortal } from './portal-context';
import { useLingering } from './lingering';
import { SectionFields } from './section-step';
import { sectionStatus } from './section-state';
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
function identityOf(adapters: AdapterUis, saved: AdapterSection | null) {
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

/**
 * One section on its own page: whether it works now, the same step as in
 * setup, and a bar at the bottom while there is something to save. After a
 * save the bar stays where the user is and follows the status: starting,
 * then working (and goes), or why it fails with the previous one offered.
 */
export function SectionEditor({ section }: { section: AdapterSide }) {
  const portal = usePortal();
  const nav = useNav();
  const { host, snapshot, online, commands } = portal;
  const { revision, settings } = snapshot;
  const saved = settings[section] ?? null;
  const previous = portal.previous[section];
  const adapters = section === 'capture' ? host.captures : host.outputs;
  const copy = stepCopy[section];
  const draft = useDraft<AdapterSection>({
    saved,
    revision,
    fresh: () => createSection(adapters),
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
  const locked = !online || busy;
  const scope = useFieldScope(locked, errors, attempted);
  const adapter = adapterFor(adapters, draft.draft.kind);
  const savedUi = saved ? adapterFor(adapters, saved.kind) : undefined;
  /** Fields to fix: their own checks block a save; Prysm's refusals not. */
  const invalid = [
    ...new Set([...Object.keys(errors.local), ...Object.keys(errors.server)]),
  ];
  const blocked = Object.keys(errors.local).length > 0;

  const stalled = useLingering(
    snapshot.target === 'screen_sync' && portal.path.kind === 'waiting',
    checkForMs,
    0,
  );
  const edges = saved ? savedUi?.edges?.(saved) : undefined;
  const status = sectionStatus(
    section,
    portal,
    identityOf(adapters, saved),
    stalled,
    {
      effects: saved ? savedUi?.lighting?.(saved).effects : '',
      leds: edges ? ledCount(edges) : undefined,
    },
  );
  const failing = status.tone === 'problem';
  const hint = failing ? savedUi?.trouble : undefined;

  const settled = status.tone === 'ok' || status.tone === 'idle';
  useEffect(() => {
    if (!watching || !settled) return;
    const timer = setTimeout(() => setWatching(false), outcomeForMs);
    return () => clearTimeout(timer);
  }, [watching, settled]);

  const normalized = () => adapter?.normalize?.(draft.draft) ?? draft.draft;

  function change(next: AdapterSection) {
    draft.setDraft(next);
    setWatching(false);
    if (phase.kind !== 'editing') setPhase({ kind: 'editing' });
  }

  /** Save `value`; `was` is offered back afterwards (`null`: nothing). */
  async function store(value: AdapterSection, was: AdapterSection | null) {
    setPhase({ kind: 'saving' });
    try {
      const next =
        section === 'capture'
          ? await commands.configureCapture(revision, value)
          : await commands.configureOutput(revision, value);
      draft.setDraft(value);
      draft.saved(value, next);
      setAttempted(false);
      portal.setPrevious({ ...portal.previous, [section]: was });
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
      if (field && showsField(formRef.current, field))
        setPhase({ kind: 'editing' });
      else
        setPhase({
          kind: 'problem',
          message: saveProblem(
            reason.kind === 'invalid'
              ? { ...reason, field: undefined }
              : reason,
          ),
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
    } else if (saved && snapshot.target !== 'off')
      line = 'Saving restarts screen sync.';
    actions = (
      <>
        <button className="primary" type="submit" disabled={locked}>
          {busy ? 'Saving…' : copy.use}
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => nav.back('settings')}
        >
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
        preview={host.preview}
        hint={hint}
        brief={outcome && failing}
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
