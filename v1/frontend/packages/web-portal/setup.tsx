import {
  useEffect,
  useId,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from 'react';
import { failureOf, same } from '@prysm/application-client';
import {
  adapterFor,
  createSection,
  kindsOf,
  type AdapterSection,
  type AdapterUis,
} from './adapters';
import {
  FieldScopeProvider,
  focusField,
  staleNotice,
  useFieldErrors,
  useFieldScope,
} from './form';
import { SectionFields } from './section-form';

const steps = [
  {
    title: 'Video source',
    intro: 'Choose where the picture comes from. You can fine-tune it later.',
  },
  {
    title: 'Your lights',
    intro:
      'Tell Prysm where the colours go and how your LEDs run around the screen.',
  },
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

function initial<S extends AdapterSection>(
  adapters: AdapterUis<S>,
  saved: S | null,
) {
  return saved ?? createSection(adapters, kindsOf(adapters)[0] ?? '');
}

function normalized<S extends AdapterSection>(
  adapters: AdapterUis<S>,
  value: S,
) {
  return adapterFor(adapters, value.kind)?.normalize?.(value) ?? value;
}

function Summary<S extends AdapterSection>({
  title,
  adapters,
  value,
  edit,
}: {
  title: string;
  adapters: AdapterUis<S>;
  value: S;
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

export interface SetupWizardProps<
  C extends AdapterSection,
  O extends AdapterSection,
> {
  captures: AdapterUis<C>;
  outputs: AdapterUis<O>;
  /** The observed sections; `null` while unconfigured. */
  capture: C | null;
  output: O | null;
  revision: number;
  disabled: boolean;
  /** Save both sections at once against `expected`. */
  save: (expected: number, capture: C, output: O) => Promise<unknown>;
  /** Start screen sync. */
  start: () => Promise<unknown>;
  /** Leave setup; `notice` says how it ended. */
  close: (notice: string) => void;
}

/**
 * Guided setup: video source, lights, review. It edits local drafts with the
 * hosts' own editors and saves capture and output together, once, at the end.
 */
export function SetupWizard<
  C extends AdapterSection,
  O extends AdapterSection,
>({
  captures,
  outputs,
  capture: savedCapture,
  output: savedOutput,
  revision,
  disabled,
  save,
  start,
  close,
}: SetupWizardProps<C, O>) {
  /** What the drafts started from; saving over a change elsewhere reloads. */
  const [base, setBase] = useState({
    capture: savedCapture,
    output: savedOutput,
  });
  const [capture, setCapture] = useState(() => initial(captures, savedCapture));
  const [output, setOutput] = useState(() => initial(outputs, savedOutput));
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
  const locked = disabled || busy;
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
    setBase({ capture: savedCapture, output: savedOutput });
    setCapture(initial(captures, savedCapture));
    setOutput(initial(outputs, savedOutput));
    setGeneration((count) => count + 1);
    errors.clearServer();
    goTo(2);
    setNotice(staleNotice);
  }
  async function finish(lighting: boolean) {
    if (locked) return;
    if (!same(savedCapture, base.capture) || !same(savedOutput, base.output)) {
      reload();
      return;
    }
    setBusy(true);
    setFailure('');
    setNotice('');
    try {
      await save(
        revision,
        normalized(captures, capture),
        normalized(outputs, output),
      );
    } catch (error) {
      setBusy(false);
      const reason = failureOf(error);
      if (reason.kind === 'stale_revision') {
        reload();
        return;
      }
      const field = reason.kind === 'invalid' ? reason.field : undefined;
      if (field) errors.setServer({ [field]: reason.message });
      setStep(stepOf(field));
      setFailure(`Not saved: ${reason.message}`);
      setFocus({ field });
      return;
    }
    if (!lighting) return close('Setup saved.');
    try {
      await start();
      close('Setup saved. Screen sync is starting.');
    } catch (error) {
      close(
        `Setup saved, but screen sync did not start: ${failureOf(error).message}`,
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
      setFailure('Correct the highlighted fields to continue.');
      setFocus({ field: invalid });
      return;
    }
    goTo(step + 1);
  }
  function change<S>(set: (value: S) => void) {
    return (value: S) => {
      set(value);
      setFailure('');
    };
  }

  let body: ReactNode;
  if (step === 0)
    body = (
      <SectionFields
        key={generation}
        section="capture"
        adapters={captures}
        value={capture}
        base={base.capture}
        onChange={change(setCapture)}
      />
    );
  else if (step === 1)
    body = (
      <SectionFields
        key={generation}
        section="output"
        adapters={outputs}
        value={output}
        base={base.output}
        onChange={change(setOutput)}
      />
    );
  else
    body = (
      <div className="setup-review">
        <Summary
          title="Video source"
          adapters={captures}
          value={capture}
          edit={() => goTo(0)}
        />
        <Summary
          title="Lights"
          adapters={outputs}
          value={output}
          edit={() => goTo(1)}
        />
      </div>
    );
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
      </p>
      <FieldScopeProvider value={scope}>{body}</FieldScopeProvider>
      {notice ? (
        <p className="notice warning" role="status">
          {notice}
        </p>
      ) : null}
      {failure ? (
        <p className="notice error" role="alert" tabIndex={-1} data-form-alert>
          {failure}
        </p>
      ) : null}
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
