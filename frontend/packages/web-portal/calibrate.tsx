import {
  useEffect,
  useId,
  useRef,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import type { LightingTarget } from '@prysm/application-client';
import type { Calibration } from '@prysm/contracts/application';
import {
  screenOf,
  screens,
  type Calibrating,
  type Closing,
  type Screen,
  type WizardPhase,
} from './calibrate-wizard';
import {
  fixLabels,
  hueGuidance,
  needsTv,
  patternLabels,
  primaryOf,
  resetOf,
  sectionOrder,
  sections,
  strengthGuidance,
  tvInstruction,
  whiteEnds,
  whiteGuidance,
  wordsOf,
} from './calibrate-copy';
import { nameProblem } from './checks';
import { RangeField, SelectField, TextField } from './fields';
import { FieldScopeProvider, focusField, type FieldScope } from './form';
import {
  correctionLimits,
  calibrationLimits,
  primaryLimits,
  endsLabel,
  signed,
  strengthEnds,
  noCorrection,
  type Primary,
} from './calibration-model';
import {
  useCalibration,
  type CalibrateProps,
  type CalibrationView,
} from './use-calibration';

/** The page each phase shows: starting stays on the start, ending on the last screen. */
const pageOf: Record<WizardPhase['kind'], 'start' | 'calibrating' | 'done'> = {
  start: 'start',
  starting: 'start',
  calibrating: 'calibrating',
  ending: 'calibrating',
  done: 'done',
};

/**
 * The Calibrate page, used on a phone while the TV shows patterns: a guided
 * walk through white, red, green and blue, then checks, on a draft
 * calibration. The draft is an ordinary calibration, created and made active at the
 * start and saved live like the Picture panel; Finish names it, Cancel
 * deletes it and makes the previous calibration active again. The work is
 * {@link useCalibration}'s; this renders it.
 */
export function CalibrateWizard(props: CalibrateProps) {
  const { target, disabled, leave } = props;
  const wizard = useCalibration(props);
  const { state, live, view, ended } = wizard;
  const { phase, problem } = state;
  const headingRef = useRef<HTMLHeadingElement>(null);
  const sectionRef = useRef<HTMLElement>(null);
  const headingId = useId();
  const page = pageOf[phase.kind];
  const screen = view?.place.screen ?? 0;
  useEffect(() => {
    headingRef.current?.focus({ preventScroll: true });
    // Each screen opens at its top, where its progress and question are,
    // also after scrolling down to the last one's controls.
    if (window.scrollY > 0) window.scrollTo({ top: 0 });
  }, [page, screen, ended]);

  const notices = (
    <>
      {problem ? (
        <p className="notice warning" role="alert">
          {problem}
        </p>
      ) : null}
      {live.failure ? (
        <p className="notice error" role="alert">
          {live.failure}
        </p>
      ) : null}
    </>
  );
  const heading = (title: string) => (
    // Keyed by screen so each screen's heading enters with its own transition.
    <h2
      key={`${page}-${screen}-${ended}`}
      id={headingId}
      ref={headingRef}
      tabIndex={-1}
    >
      {title}
    </h2>
  );

  let content: ReactNode;
  if (phase.kind === 'done')
    content = (
      <>
        <div className="card-heading">
          <p className="eyebrow">Calibrate colours</p>
        </div>
        {heading(phase.title)}
        <p className="intro" role="status">
          {phase.message}
        </p>
        <div className="actions">
          <button type="button" className="primary" onClick={leave}>
            Back to Prysm
          </button>
        </div>
      </>
    );
  else if (phase.kind === 'start' || phase.kind === 'starting')
    content = (
      <StartScreen
        heading={heading}
        notices={notices}
        calibrations={wizard.calibrations}
        from={phase.from}
        choose={wizard.choose}
        scope={live.scope}
        target={target}
        locked={disabled || phase.kind === 'starting'}
        starting={phase.kind === 'starting'}
        start={() => void wizard.start()}
        leave={leave}
      />
    );
  else
    content = (
      <StepScreen
        wizard={wizard}
        view={phase.kind === 'ending' ? phase.last : phase}
        heading={heading}
        notices={notices}
        target={target}
        disabled={disabled}
        finish={() => {
          // The field says what is wrong, in its own note.
          if (wizard.nameProblem)
            focusField(sectionRef.current, 'calibration.name');
          else void wizard.finish();
        }}
      />
    );
  return (
    <section
      ref={sectionRef}
      className="card calibrate"
      aria-labelledby={headingId}
    >
      {content}
    </section>
  );
}

/** Before the start: what to copy, and why the start may not work yet. */
function StartScreen({
  heading,
  notices,
  calibrations,
  from,
  choose,
  scope,
  target,
  locked,
  starting,
  start,
  leave,
}: {
  heading: (title: string) => ReactNode;
  notices: ReactNode;
  calibrations: readonly Calibration[];
  from: string;
  choose: (from: string) => void;
  scope: FieldScope;
  target: LightingTarget;
  locked: boolean;
  starting: boolean;
  start: () => void;
  leave: () => void;
}) {
  const full = calibrations.length >= calibrationLimits.count;
  return (
    <>
      <div className="card-heading">
        <p className="eyebrow">Calibrate colours</p>
        {/* While starting, the start must finish or undo itself first. */}
        <button
          type="button"
          className="quiet"
          disabled={starting}
          onClick={leave}
        >
          Leave
        </button>
      </div>
      {heading('Match the lights to your TV')}
      <p className="intro">
        The TV shows test patterns while you adjust the colours here. Look at
        the screen and the wall, not at this phone.
      </p>
      <p className="help">
        Patterns are compared in the TV’s current picture mode, so calibrate in
        the mode you watch in.
      </p>
      {target !== 'processed' ? (
        <p className="notice warning" role="status">
          {needsTv}
        </p>
      ) : null}
      {full ? (
        <p className="notice warning">
          You have the most calibrations Prysm keeps ({calibrationLimits.count}
          ). Delete one to calibrate.
        </p>
      ) : null}
      <FieldScopeProvider value={scope}>
        {calibrations.length ? (
          <SelectField
            path="calibration.from"
            label="Start from"
            help="Calibrating works on a copy; the original stays as it is."
            value={from}
            options={[
              { value: '', label: 'New calibration' },
              ...calibrations.map((calibration) => ({
                value: calibration.id,
                label: calibration.name,
              })),
            ]}
            onChange={choose}
          />
        ) : null}
      </FieldScopeProvider>
      {notices}
      <div className="actions">
        <button
          type="button"
          className="primary"
          disabled={locked || full || target !== 'processed'}
          onClick={start}
        >
          {starting ? 'Starting…' : 'Start'}
        </button>
      </div>
    </>
  );
}

/**
 * The top line: where you are, and Cancel, quiet like Setup's Leave setup.
 * Cancel discards work, so with changes it first asks in the same place;
 * nothing below moves.
 */
function StepHeading({
  confirming,
  closing,
  disabled,
  cancel,
  keep,
  discard,
}: {
  confirming: boolean;
  /** Finish, Cancel or Leave is under way. */
  closing: Closing | null;
  disabled: boolean;
  cancel: () => void;
  keep: () => void;
  discard: () => void;
}) {
  const questionId = useId();
  const keepRef = useRef<HTMLButtonElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  /** Focus follows the question in and back out, never on the first draw. */
  const askedRef = useRef(false);
  const escape = (event: KeyboardEvent) => {
    if (event.key === 'Escape' && !closing) keep();
  };
  useEffect(() => {
    if (confirming) keepRef.current?.focus();
    else if (askedRef.current) cancelRef.current?.focus();
    askedRef.current ||= confirming;
  }, [confirming]);
  return (
    <div className="card-heading">
      {confirming ? (
        <div
          className="calibrate-confirm"
          role="group"
          aria-labelledby={questionId}
        >
          <p id={questionId}>Discard your changes?</p>
          <button
            ref={keepRef}
            type="button"
            className="quiet"
            disabled={closing !== null}
            onClick={keep}
            onKeyDown={escape}
          >
            Keep
          </button>
          <button
            type="button"
            className="quiet danger"
            disabled={disabled || closing !== null}
            aria-busy={closing === 'discard' || undefined}
            onClick={discard}
            onKeyDown={escape}
          >
            Discard
          </button>
        </div>
      ) : (
        <>
          <p className="eyebrow">Calibrate colours</p>
          <button
            ref={cancelRef}
            type="button"
            className="quiet"
            disabled={disabled || closing !== null}
            aria-busy={closing === 'discard' || undefined}
            onClick={cancel}
          >
            Cancel calibration
          </button>
        </>
      )}
    </div>
  );
}

/** Where the screen is in the walk: its section, and its place in a section of several. */
function StepProgress({ current }: { current: Screen }) {
  const sectionIndex = sectionOrder.indexOf(current.section);
  const inSection = screens.filter((item) => item.section === current.section);
  const position = inSection.indexOf(current);
  return (
    <ol className="setup-steps" aria-label="Calibration progress">
      {sectionOrder.map((id, index) => (
        <li
          key={id}
          className={index < sectionIndex ? 'done' : undefined}
          aria-current={index === sectionIndex ? 'step' : undefined}
        >
          {sections[id]}
          {index === sectionIndex && inSection.length > 1
            ? ` · ${position + 1} of ${inSection.length}`
            : null}
        </li>
      ))}
    </ol>
  );
}

/** The TV no longer shows the patterns: the draft is kept, Resume or Leave. */
function EndedNotice({
  wizard,
  closing,
  heading,
  notices,
  target,
  disabled,
}: {
  wizard: CalibrationView;
  closing: Closing | null;
  heading: (title: string) => ReactNode;
  notices: ReactNode;
  target: LightingTarget;
  disabled: boolean;
}) {
  return (
    <>
      {heading('Calibration ended')}
      <p className="notice warning" role="alert">
        The TV no longer shows the patterns. Your changes so far are kept in “
        {wizard.draft ? wizard.draft.name : 'your calibration'}”.
        {target !== 'processed' ? ` ${tvInstruction}` : ''}
      </p>
      {notices}
      <div className="actions">
        <button
          type="button"
          disabled={closing !== null}
          aria-busy={closing === 'leave' || undefined}
          onClick={() => void wizard.leave()}
        >
          Leave
        </button>
        <button
          type="button"
          className="primary"
          disabled={disabled || closing !== null}
          onClick={wizard.resume}
        >
          Resume
        </button>
      </div>
    </>
  );
}

/** The screen's sliders, its links back to what a check is made of, and the name. */
function StepControls({
  wizard,
  view,
  current,
  disabled,
}: {
  wizard: CalibrationView;
  view: Calibrating;
  current: Screen;
  disabled: boolean;
}) {
  const { draft, live, path } = wizard;
  const fixesId = useId();
  if (!draft)
    return (
      <p className="notice warning">
        This calibration was deleted elsewhere. Cancel to finish.
      </p>
    );
  const values = draft.correction;
  const range = (
    control: string,
    ends: readonly [string, string],
    help: string,
    value: number,
    onChange: (value: number) => void,
    limits: { min: number; max: number; step: number },
  ) => (
    <RangeField
      key={control}
      className={`slider-${control.replace('.', '-')}`}
      path={`${path}.${control}`}
      label={endsLabel(ends)}
      help={help}
      ends={ends}
      value={value}
      onChange={onChange}
      onPress={live.slider.press}
      onRelease={live.slider.release}
      format={signed}
      {...limits}
    />
  );
  const slider = (control: keyof typeof whiteEnds) =>
    range(
      control,
      whiteEnds[control],
      whiteGuidance[control],
      values[control],
      (value) => wizard.correct({ [control]: value }, control),
      correctionLimits[control],
    );
  const primary = primaryOf(current.section);
  const primarySlider = (
    name: Primary,
    control: keyof typeof primaryLimits,
    ends: readonly [string, string],
  ) =>
    range(
      `${name}.${control}`,
      ends,
      control === 'hue' ? hueGuidance(ends) : strengthGuidance,
      values[name][control],
      (value) =>
        wizard.correct(
          { [name]: { ...values[name], [control]: value } },
          `${name}.${control}`,
        ),
      primaryLimits[control],
    );
  return (
    <FieldScopeProvider value={live.scope}>
      <div key={live.generation} className="calibrate-controls">
        {current.section === 'white' ? (
          <>
            {slider('warmth')}
            {slider('tint')}
          </>
        ) : null}
        {primary ? (
          <>
            {primarySlider(primary.name, 'hue', primary.hue)}
            {primarySlider(primary.name, 'strength', strengthEnds)}
          </>
        ) : null}
        {current.fixes ? (
          <div
            className="calibrate-fixes"
            role="group"
            aria-labelledby={fixesId}
          >
            <p id={fixesId}>Not quite?</p>
            {current.fixes.map((fix) => (
              <button
                key={fix}
                type="button"
                disabled={disabled}
                onClick={() => wizard.go(screenOf(fix), view.place.screen)}
              >
                {fixLabels[fix]}
              </button>
            ))}
          </div>
        ) : null}
        {current.section === 'review' ? (
          <TextField
            path="calibration.name"
            label="Name"
            help="A name you will recognise, such as Movie night or PS5."
            value={view.name ?? draft.name}
            required
            check={(value) => nameProblem(value.trim(), calibrationLimits.name)}
            onChange={wizard.name}
          />
        ) : null}
      </div>
    </FieldScopeProvider>
  );
}

/** One pattern on the TV and its controls, or the notice that calibration ended. */
function StepScreen({
  wizard,
  view,
  heading,
  notices,
  target,
  disabled,
  finish,
}: {
  wizard: CalibrationView;
  /** The calibrating screen, also while calibration is being ended. */
  view: Calibrating;
  heading: (title: string) => ReactNode;
  notices: ReactNode;
  target: LightingTarget;
  /** The portal cannot take commands now. */
  disabled: boolean;
  finish: () => void;
}) {
  const { draft } = wizard;
  const { place, closing } = view;
  const { screen, returnTo, uncorrected } = place;
  const current = screens[screen] ?? screens[0]!;
  const words = wordsOf(current);
  const reset = resetOf(
    current.section,
    draft ? draft.correction : noCorrection,
  );
  const review = current.section === 'review';
  return (
    <>
      <StepHeading
        confirming={view.confirming}
        closing={closing}
        disabled={disabled}
        cancel={wizard.cancel}
        keep={wizard.keep}
        discard={() => void wizard.discard()}
      />
      <StepProgress current={current} />
      {wizard.ended ? (
        <EndedNotice
          wizard={wizard}
          closing={closing}
          heading={heading}
          notices={notices}
          target={target}
          disabled={disabled}
        />
      ) : (
        <>
          {heading(words.title)}
          {/* While the TV is still changing, the swatch waits dimmed. */}
          <p
            className="calibrate-pattern"
            data-pattern={current.pattern}
            role="status"
            aria-busy={wizard.pending || undefined}
          >
            On your TV: {patternLabels[current.pattern]}
          </p>
          <p className="intro">{words.lead}</p>
          <StepControls
            wizard={wizard}
            view={view}
            current={current}
            disabled={disabled}
          />
          {/* Compare and Reset in one row: used often, always in the same place. */}
          <div className="calibrate-tools">
            <button
              type="button"
              className="compare"
              aria-pressed={uncorrected}
              disabled={disabled}
              onClick={wizard.compare}
            >
              Show without correction
            </button>
            {reset && draft ? (
              <button
                type="button"
                className="quiet"
                disabled={disabled || !reset.changed}
                onClick={() => wizard.correct(reset.to, reset.control)}
              >
                Reset
              </button>
            ) : null}
          </div>
          {notices}
          <div className="actions">
            {screen > 0 && returnTo === null ? (
              <button
                type="button"
                disabled={disabled}
                onClick={() => wizard.go(screen - 1)}
              >
                Back
              </button>
            ) : null}
            {review ? (
              <button
                type="button"
                className="primary"
                disabled={disabled || !draft || closing !== null}
                aria-busy={closing === 'finish' || undefined}
                onClick={finish}
              >
                Finish
              </button>
            ) : (
              <button
                type="button"
                className="primary"
                disabled={disabled}
                onClick={() => wizard.go(returnTo ?? screen + 1)}
              >
                Looks right
              </button>
            )}
          </div>
        </>
      )}
    </>
  );
}
