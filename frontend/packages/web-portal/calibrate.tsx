import {
  useEffect,
  useId,
  useRef,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import {
  screenOf,
  screens,
  type Calibrating,
  type Screen,
} from './calibrate-wizard';
import {
  fixLabels,
  needsTv,
  patternLabels,
  primaryOf,
  resetOf,
  sectionOrder,
  sections,
  tvInstruction,
  wordsOf,
} from './calibrate-copy';
import { nameProblem } from './checks';
import { WhiteSliders, PrimarySliders } from './correction-sliders';
import { useNav } from './portal-context';
import { SelectField, TextField } from './fields';
import { FieldScopeProvider, focusField } from './form';
import { calibrationLimits, noCorrection } from './calibration-model';
import { useCalibration, type CalibrationView } from './use-calibration';

interface ScreenProps {
  wizard: CalibrationView;
  heading: (title: string) => ReactNode;
  notices: ReactNode;
}

/**
 * The Calibrate page, used on a phone while the TV shows patterns: a guided
 * walk through white, red, green and blue, then checks, on a draft
 * calibration. The draft is an ordinary calibration, created and made active at the
 * start and saved live like the Picture panel; Finish names it, Cancel
 * deletes it and makes the previous calibration active again. The work is
 * {@link useCalibration}'s; this renders it.
 */
export function CalibrateWizard() {
  const { backToOpener: leave } = useNav();
  const wizard = useCalibration(leave);
  const { view, problem, failure, ended } = wizard;
  const headingRef = useRef<HTMLHeadingElement>(null);
  const sectionRef = useRef<HTMLElement>(null);
  const headingId = useId();
  const page = view.kind === 'starting' ? 'start' : view.kind;
  const screen = view.kind === 'calibrating' ? view.place.screen : 0;
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
      {failure ? (
        <p className="notice error" role="alert">
          {failure}
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

  const screenProps = { wizard, heading, notices };
  let content: ReactNode;
  if (view.kind === 'done')
    content = (
      <>
        <div className="card-heading">
          <p className="eyebrow">Calibrate colours</p>
        </div>
        {heading(view.title)}
        <p className="intro" role="status">
          {view.message}
        </p>
        <div className="actions">
          <button type="button" className="primary" onClick={leave}>
            Back to Prysm
          </button>
        </div>
      </>
    );
  else if (view.kind === 'start' || view.kind === 'starting')
    content = <StartScreen {...screenProps} phase={view} leave={leave} />;
  else
    content = (
      <StepScreen
        {...screenProps}
        view={view}
        finish={() => {
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
  wizard,
  phase,
  heading,
  notices,
  leave,
}: ScreenProps & {
  phase: Extract<CalibrationView['view'], { kind: 'start' | 'starting' }>;
  leave: () => void;
}) {
  const { calibrations, scope, target } = wizard;
  const starting = phase.kind === 'starting';
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
      {target !== 'screen_sync' ? (
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
            value={phase.from}
            options={[
              { value: '', label: 'New calibration' },
              ...calibrations.map((calibration) => ({
                value: calibration.id,
                label: calibration.name,
              })),
            ]}
            onChange={wizard.choose}
          />
        ) : null}
      </FieldScopeProvider>
      {notices}
      <div className="actions">
        <button
          type="button"
          className="primary"
          disabled={
            scope.disabled || starting || full || target !== 'screen_sync'
          }
          onClick={() => void wizard.start()}
        >
          {starting ? 'Starting…' : 'Start'}
        </button>
      </div>
    </>
  );
}

function StepHeading({
  wizard,
  view,
}: {
  wizard: CalibrationView;
  view: Calibrating;
}) {
  const { confirming, closing } = view;
  const { cancel, keep } = wizard;
  const { disabled } = wizard.scope;
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
            onClick={() => void wizard.discard()}
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
  view,
  heading,
  notices,
}: ScreenProps & { view: Calibrating }) {
  const { closing } = view;
  const { target } = wizard;
  const { disabled } = wizard.scope;
  return (
    <>
      {heading('Calibration ended')}
      <p className="notice warning" role="alert">
        The TV no longer shows the patterns. Your changes so far are kept in “
        {wizard.editor?.calibration.name ?? 'your calibration'}”.
        {target !== 'screen_sync' ? ` ${tvInstruction}` : ''}
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

function StepControls({
  wizard,
  view,
  current,
}: {
  wizard: CalibrationView;
  view: Calibrating;
  current: Screen;
}) {
  const { editor, scope } = wizard;
  const { disabled } = scope;
  const fixesId = useId();
  if (!editor)
    return (
      <p className="notice warning">
        This calibration was deleted elsewhere. Cancel to finish.
      </p>
    );
  const { calibration: draft, sliders } = editor;
  const primary = primaryOf(current.section);
  return (
    <FieldScopeProvider value={scope}>
      <div key={wizard.generation} className="calibrate-controls">
        {current.section === 'white' && <WhiteSliders {...sliders} guided />}
        {primary && <PrimarySliders {...sliders} primary={primary} guided />}
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
  finish,
}: ScreenProps & {
  view: Calibrating;
  finish: () => void;
}) {
  const { editor } = wizard;
  const { disabled } = wizard.scope;
  const { place, closing } = view;
  const { screen, returnTo, uncorrected } = place;
  const current = screens[screen] ?? screens[0]!;
  const words = wordsOf(current);
  const reset = resetOf(
    current.section,
    editor?.calibration.correction ?? noCorrection,
  );
  const review = current.section === 'review';
  return (
    <>
      <StepHeading wizard={wizard} view={view} />
      <StepProgress current={current} />
      {wizard.ended ? (
        <EndedNotice
          wizard={wizard}
          view={view}
          heading={heading}
          notices={notices}
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
          <StepControls wizard={wizard} view={view} current={current} />
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
            {reset && editor ? (
              <button
                type="button"
                className="quiet"
                disabled={disabled || !reset.changed}
                onClick={() => editor.sliders.change(reset.to, reset.control)}
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
                disabled={disabled || !editor || closing !== null}
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
