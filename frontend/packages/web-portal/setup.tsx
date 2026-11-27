import { useEffect, useId, useRef, useState, type FormEvent } from 'react';
import { failureOf, plainFailure, same } from '@prysm/application-client';
import {
  adapterFor,
  createSection,
  refusal,
  type AdapterSection,
  type AdapterUis,
} from './adapters';
import {
  FieldScopeProvider,
  focusField,
  saveProblem,
  staleNotice,
  useFieldErrors,
  useFieldScope,
} from './form';
import { usePortal } from './portal-context';
import { SectionFields } from './section-step';
import { stepCopy } from './step-copy';

const sides = ['capture', 'output'] as const;
const steps = [
  ...sides.map((side) => stepCopy[side]),
  {
    title: 'Review and start',
    intro:
      'Check your setup. Nothing is saved until you finish, and you can finish while your devices are offline.',
  },
] as const;

/** The step that shows the control for a server error's `field`. */
function stepOf(field: string | undefined) {
  switch (field?.split('.')[0]) {
    case 'capture':
      return 0;
    case 'output':
      return 1;
    default:
      return 2;
  }
}

function normalized(adapters: AdapterUis, value: AdapterSection) {
  return adapterFor(adapters, value.kind)?.normalize?.(value) ?? value;
}

function Summary({
  title,
  adapters,
  value,
  edit,
}: {
  title: string;
  adapters: AdapterUis;
  value: AdapterSection;
  edit: () => void;
}) {
  const adapter = adapterFor(adapters, value.kind);
  return (
    <div className="review-item">
      <h3>{title}</h3>
      <p>
        <strong>{adapter?.label ?? value.kind}</strong>
        {adapter?.summary ? (
          <span className="review-detail">
            {adapter.summary(normalized(adapters, value))}
          </span>
        ) : null}
      </p>
      <button
        type="button"
        onClick={edit}
        aria-label={`Change ${title.toLowerCase()}`}
      >
        Change
      </button>
    </div>
  );
}

/**
 * Guided setup: video source, lights, review. It edits local drafts with the
 * hosts' own editors and saves capture and output together, once, at the end.
 */
export function SetupWizard({ close }: { close: (notice: string) => void }) {
  const {
    host: { captures, outputs },
    snapshot: { settings, revision },
    online,
    commands,
  } = usePortal();
  const adapters = { capture: captures, output: outputs };
  const saved = {
    capture: settings.capture ?? null,
    output: settings.output ?? null,
  };
  const fresh = () => ({
    capture: saved.capture ?? createSection(captures),
    output: saved.output ?? createSection(outputs),
  });
  /** Save both sections only if neither has changed elsewhere. */
  const [base, setBase] = useState(saved);
  const [draft, setDraft] = useState(fresh);
  /** Changes when the drafts are replaced, to remount their inputs. */
  const [generation, setGeneration] = useState(0);
  const [step, setStep] = useState(0);
  const errors = useFieldErrors();
  const [attempted, setAttempted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState('');
  const [notice, setNotice] = useState('');
  const formRef = useRef<HTMLFormElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  /**
   * Once rendered: the step heading, or a field (else the alert) after an
   * error. Each request is a new object, so the effect runs once per request.
   */
  const [focus, setFocus] = useState<
    { heading: true } | { field: string | undefined }
  >({ heading: true });
  useEffect(() => {
    if ('heading' in focus) headingRef.current?.focus();
    else focusField(formRef.current, focus.field);
  }, [focus]);
  const headingId = useId();
  const locked = !online || busy;
  const scope = useFieldScope(locked, errors, attempted);
  const current = steps[step] ?? steps[0];

  function goTo(next: number) {
    setStep(next);
    setAttempted(false);
    setFailure('');
    setNotice('');
    setFocus({ heading: true });
  }
  function reload() {
    setBase(saved);
    setDraft(fresh());
    setGeneration((count) => count + 1);
    errors.clearServer();
    goTo(2);
    setNotice(staleNotice);
  }
  async function finish(lighting: boolean) {
    if (locked) return;
    if (!same(saved, base)) {
      reload();
      return;
    }
    setBusy(true);
    setFailure('');
    setNotice('');
    try {
      await commands.configureSetup(
        revision,
        normalized(captures, draft.capture),
        normalized(outputs, draft.output),
      );
    } catch (error) {
      setBusy(false);
      const reason = failureOf(error);
      if (reason.kind === 'stale_revision') {
        reload();
        return;
      }
      const field = reason.kind === 'invalid' ? reason.field : undefined;
      if (field) {
        const side = field.startsWith('capture') ? 'capture' : 'output';
        const adapter = adapterFor(adapters[side], draft[side].kind);
        errors.setServer({ [field]: refusal(adapter, field) });
      }
      setStep(stepOf(field));
      setFailure(saveProblem(reason));
      setFocus({ field });
      return;
    }
    if (!lighting) return close('Setup saved.');
    try {
      await commands.setTarget('screen_sync');
      close('Setup saved. Screen sync is starting.');
    } catch (error) {
      close(
        `Setup saved, but screen sync didn’t start. ${plainFailure(failureOf(error), 'screen_sync', 'Device effects')}`,
      );
    }
  }
  function submit(event: FormEvent) {
    event.preventDefault();
    if (locked) return;
    if (step === 2) {
      void finish(true);
      return;
    }
    setAttempted(true);
    const [invalid] = Object.keys(errors.local);
    if (invalid) {
      setFailure('Check the marked field to continue.');
      setFocus({ field: invalid });
      return;
    }
    goTo(step + 1);
  }
  const side = sides[step];
  let tone: 'error' | 'warning' | undefined;
  if (failure) tone = 'error';
  else if (notice) tone = 'warning';
  let submitLabel = 'Save and start screen sync';
  if (step < 2) submitLabel = 'Next';
  else if (busy) submitLabel = 'Saving…';

  return (
    <form
      ref={formRef}
      className="card setup"
      aria-labelledby={headingId}
      aria-busy={busy}
      noValidate
      onSubmit={submit}
    >
      <div className="card-heading">
        <p className="eyebrow">
          Set up Prysm · step {step + 1} of {steps.length}
        </p>
        <button
          type="button"
          className="quiet"
          onClick={() => close('Setup closed; nothing was saved.')}
        >
          Leave setup
        </button>
      </div>
      <ol className="setup-steps" aria-label="Setup progress">
        {steps.map(({ title }, index) => (
          <li
            key={title}
            className={index < step ? 'done' : undefined}
            aria-current={index === step ? 'step' : undefined}
          >
            {title}
          </li>
        ))}
      </ol>
      {/* Keyed by step so each step's heading enters with its own transition. */}
      <h2 key={step} id={headingId} ref={headingRef} tabIndex={-1}>
        {current.title}
      </h2>
      <p key={`intro-${step}`} className="intro">
        {current.intro}
        {step === 0
          ? ` ${adapterFor(captures, draft.capture.kind)?.setup ?? ''}`
          : null}
      </p>
      <FieldScopeProvider value={scope}>
        {side ? (
          <SectionFields
            key={generation}
            section={side}
            adapters={adapters[side]}
            value={draft[side]}
            base={base[side]}
            onChange={(value) => {
              setDraft((current) => ({ ...current, [side]: value }));
              setFailure('');
            }}
          />
        ) : (
          <div className="setup-review">
            {sides.map((section, index) => (
              <Summary
                key={section}
                title={section === 'capture' ? 'Video source' : 'Lights'}
                adapters={adapters[section]}
                value={draft[section]}
                edit={() => goTo(index)}
              />
            ))}
          </div>
        )}
      </FieldScopeProvider>
      <p
        className="form-status"
        data-tone={tone}
        role="status"
        tabIndex={-1}
        data-form-alert
      >
        {failure || notice}
      </p>
      <div className="actions">
        {step > 0 ? (
          <button type="button" disabled={busy} onClick={() => goTo(step - 1)}>
            Back
          </button>
        ) : null}
        {step === 2 ? (
          <button
            type="button"
            disabled={locked}
            onClick={() => void finish(false)}
          >
            Save only
          </button>
        ) : null}
        <button className="primary" type="submit" disabled={locked}>
          {submitLabel}
        </button>
      </div>
    </form>
  );
}
