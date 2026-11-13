import { useEffect, useId, useRef, useState, type FormEvent } from 'react';
import { failureOf } from '@prysm/application-client';
import {
  adapterFor,
  createSection,
  kindsOf,
  type AdapterSection,
  type AdapterUis,
} from './adapters';
import { useDraft } from './draft';
import { SelectField } from './fields';
import {
  FieldScopeProvider,
  focusField,
  staleNotice,
  useFieldErrors,
  useFieldScope,
  useFieldScopeContext,
} from './form';

/**
 * The kind selector (when the host registers more than one kind) and the
 * kind's editor. Switching kinds keeps what was typed for the other kinds and
 * drops the server's errors for the section. Used inside a field scope.
 */
export function SectionFields<S extends AdapterSection>({
  section,
  adapters,
  value,
  base,
  onChange,
}: {
  section: 'capture' | 'output';
  adapters: AdapterUis<S>;
  value: S;
  /** The saved section, offered again when its kind is chosen. */
  base: S | null;
  onChange: (next: S) => void;
}) {
  const scope = useFieldScopeContext();
  /** Drafts of other kinds, so switching back keeps what was typed. */
  const [others, setOthers] = useState<Record<string, S>>({});
  const kinds = kindsOf(adapters);
  function switchKind(kind: S['kind']) {
    if (kind === value.kind) return;
    setOthers({ ...others, [value.kind]: value });
    scope.edited(section);
    onChange(
      others[kind] ??
        (base?.kind === kind ? base : createSection(adapters, kind)),
    );
  }
  const Editor = adapterFor(adapters, value.kind)?.Editor;
  return (
    <>
      {kinds.length > 1 ? (
        <SelectField
          path={`${section}.kind`}
          label={section === 'capture' ? 'Capture from' : 'Send colours to'}
          value={value.kind}
          options={kinds.map((kind) => ({
            value: kind,
            label: adapterFor(adapters, kind)?.label ?? kind,
          }))}
          onChange={switchKind}
        />
      ) : null}
      {Editor ? (
        <Editor key={value.kind} value={value} onChange={onChange} />
      ) : (
        <p className="notice warning">
          This host has no editor for {section} kind “{value.kind}”.
        </p>
      )}
    </>
  );
}

export interface SectionFormProps<S extends AdapterSection> {
  section: 'capture' | 'output';
  title: string;
  description: string;
  adapters: AdapterUis<S>;
  /** The observed section; `null` while unconfigured. */
  saved: S | null;
  revision: number;
  disabled: boolean;
  /** Save the whole section against `expected`; resolves with the new revision. */
  save: (expected: number, value: S) => Promise<number>;
}

/**
 * One adapter section with an explicit Save: kind selector (when the host
 * registers more than one kind), the kind's editor, dirty state, reset,
 * field errors and the stale-settings reload.
 */
export function SectionForm<S extends AdapterSection>({
  section,
  title,
  description,
  adapters,
  saved,
  revision,
  disabled,
  save,
}: SectionFormProps<S>) {
  const draft = useDraft<S>({
    saved,
    revision,
    fresh: () => createSection(adapters, kindsOf(adapters)[0] ?? ''),
  });
  const errors = useFieldErrors();
  const [busy, setBusy] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [failure, setFailure] = useState('');
  const [notice, setNotice] = useState('');
  const formRef = useRef<HTMLFormElement>(null);
  /**
   * After a refused save, focus this field (or the alert) once it renders.
   * Each request is a new object, so the effect runs once per request.
   */
  const [focus, setFocus] = useState<{ field?: string } | null>(null);
  useEffect(() => {
    if (focus) focusField(formRef.current, focus.field);
  }, [focus]);
  const headingId = useId();
  const statusId = useId();
  const adapter = adapterFor(adapters, draft.draft.kind);
  const locked = disabled || busy || draft.syncing;
  const scope = useFieldScope(locked, errors, attempted);
  const invalid = Object.keys(errors.local).length > 0;

  function change(next: S) {
    draft.setDraft(next);
    setFailure('');
    setNotice('');
  }
  function reset() {
    draft.reload();
    setAttempted(false);
    errors.clearServer();
    setFailure('');
    setNotice('');
  }
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (locked) return;
    setAttempted(true);
    if (invalid) {
      setFailure('Correct the highlighted fields before saving.');
      setFocus({ field: Object.keys(errors.local)[0] });
      return;
    }
    if (draft.conflicted) {
      reset();
      setNotice(staleNotice);
      return;
    }
    const value = adapter?.normalize?.(draft.draft) ?? draft.draft;
    setBusy(true);
    setFailure('');
    setNotice('');
    try {
      const next = await save(revision, value);
      draft.setDraft(value);
      draft.saved(value, next);
      setAttempted(false);
      setNotice('Saved.');
    } catch (error) {
      const reason = failureOf(error);
      if (reason.kind === 'stale_revision') {
        reset();
        setNotice(staleNotice);
      } else {
        if (reason.kind === 'invalid' && reason.field)
          errors.setServer({ [reason.field]: reason.message });
        setFailure(`Not saved: ${reason.message}`);
        setFocus({ field: reason.field });
      }
    } finally {
      setBusy(false);
    }
  }

  let status = 'Saved';
  if (busy || draft.syncing) status = 'Saving…';
  else if (draft.unconfigured) status = 'Not configured';
  else if (draft.dirty) status = 'Unsaved changes';
  return (
    <form
      ref={formRef}
      className="card section-form"
      aria-labelledby={headingId}
      aria-describedby={statusId}
      aria-busy={busy || draft.syncing}
      noValidate
      onSubmit={(event) => void submit(event)}
    >
      <div className="card-heading">
        <h2 id={headingId}>{title}</h2>
        <span
          id={statusId}
          className={`badge${draft.dirty || draft.unconfigured ? ' attention' : ''}`}
        >
          {status}
        </span>
      </div>
      <p className="intro">{description}</p>
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
      {draft.conflicted && draft.dirty ? (
        <p className="notice warning" role="status">
          Changed elsewhere since you started editing. Saving will load the
          current settings instead.
        </p>
      ) : null}
      {notice ? (
        <p
          className={`notice${notice === staleNotice ? ' warning' : ''}`}
          role="status"
        >
          {notice}
        </p>
      ) : null}
      {failure ? (
        <p className="notice error" role="alert" tabIndex={-1} data-form-alert>
          {failure}
        </p>
      ) : null}
      <div className="actions">
        <button
          type="button"
          onClick={reset}
          disabled={locked || (!draft.dirty && !invalid)}
        >
          Reset
        </button>
        <button
          className="primary"
          type="submit"
          disabled={locked || (!draft.dirty && !draft.unconfigured)}
        >
          {busy ? 'Saving…' : `Save ${section} settings`}
        </button>
      </div>
    </form>
  );
}
