import { useEffect, useId, useReducer, useRef, type ReactNode } from 'react';
import {
  failureOf,
  type CalibrationPattern,
  type LightingTarget,
  type Pattern,
} from '@prysm/application-client';
import type {
  Calibration,
  CorrectionControls,
  Processing,
} from '@prysm/contracts/application';
import {
  initialWizard,
  screenOf,
  screens,
  wizardReducer,
  type Adjustable,
  type Screen,
  type Section,
  type WizardAction,
} from './calibrate-wizard';
import { nameProblem } from './checks';
import { RangeField, SelectField, TextField } from './fields';
import { FieldScopeProvider, focusField, type FieldScope } from './form';
import { useLiveProcessing, withCalibrations } from './live-processing';
import {
  correctionLimits,
  correctionOf,
  corrections,
  freeName,
  nameOf,
  newCalibrationId,
  noCorrection,
  calibrationLimits,
  primaries,
  primaryLimits,
  endsLabel,
  signed,
  strengthEnds,
  type Primary,
} from './calibration-model';

/** What the TV shows for each pattern. */
const patternLabels: Record<Pattern, string> = {
  'gray-192': 'light grey',
  red: 'red',
  yellow: 'yellow',
  green: 'green',
  cyan: 'cyan',
  blue: 'blue',
  magenta: 'magenta',
  'blue-pair': 'blue, moderate left, vivid right',
  'red-pair': 'red, moderate left, vivid right',
  'green-pair': 'green, moderate left, vivid right',
  coast: 'peach left, teal right',
  garden: 'rose left, sage right',
  dusk: 'amber left, violet right',
  'review-grid': 'a grid of colours',
};

/** Each section's name in the progress bar. */
const sections: Record<Section, string> = {
  white: 'White',
  red: 'Red',
  green: 'Green',
  blue: 'Blue',
  checks: 'Checks',
  review: 'Review',
};
const sectionOrder = Object.keys(sections) as Section[];

const primaryOf = (section: Section) =>
  primaries.find((primary) => primary.name === section);

/** A screen's one question and how to answer it. */
function wordsOf(screen: Screen): { title: string; lead: string } {
  const primary = primaryOf(screen.section);
  if (primary) {
    const colour = primary.label.toLowerCase();
    return {
      title: `Does the ${colour} on the wall match the screen?`,
      lead: 'Compare each half of the TV with the wall behind it.',
    };
  }
  switch (screen.section) {
    case 'white':
      return {
        title: 'Does the wall look as neutral as the screen?',
        lead: 'Compare only the colour, not the brightness. Small changes are enough.',
      };
    case 'checks': {
      const fixes = screen.fixes ?? [];
      // A mixed scene can be off anywhere; a secondary in its two primaries.
      if (fixes.includes('white'))
        return {
          title: 'Do both colours on the wall match the screen?',
          lead: 'Compare each side of the wall with the same side of the TV. If one looks off, adjust white or its closest colour.',
        };
      const colour = patternLabels[screen.pattern];
      return {
        title: `Does the ${colour} on the wall match the screen?`,
        lead: `${colour[0]!.toUpperCase()}${colour.slice(1)} mixes ${fixes.join(' and ')}. If it looks off on the wall, adjust one of them.`,
      };
    }
    default:
      return {
        title: 'Name your calibration',
        lead: 'Check the glow around the whole screen. When it looks right, name it and finish.',
      };
  }
}

/** This screen's own controls back to 0, and whether any is not; `null` without controls. */
function resetOf(
  section: Section,
  values: CorrectionControls,
): { to: Partial<CorrectionControls>; changed: boolean } | null {
  if (section === 'white')
    return {
      to: { warmth: 0, tint: 0 },
      changed: values.warmth !== 0 || values.tint !== 0,
    };
  const primary = primaryOf(section);
  if (!primary) return null;
  const { hue, strength } = values[primary.name];
  return {
    to: { [primary.name]: noCorrection[primary.name] },
    changed: hue !== 0 || strength !== 0,
  };
}

/** “More orange” → “orange”: the colour a slider's end adds. */
const leaning = (end: string) => end.replace(/^More /, '').toLowerCase();

/** What to do with a slider, from what the wall shows: one line under it. */
const hueGuidance = ([start, end]: readonly [string, string]) =>
  `Too ${leaning(end)}? Toward ${leaning(start)}. Too ${leaning(start)}? Toward ${leaning(end)}.`;
const whiteGuidance = {
  warmth: 'Looks blue? Warm it up. Looks yellow? Cool it down.',
  tint: 'Looks green? Toward pink. Looks pink? Toward green.',
} as const;
const strengthGuidance = `Too vivid? Toward ${leaning(strengthEnds[0])}. Too pale? Toward ${leaning(strengthEnds[1])}.`;

/** A check's link back to what it is made of. */
const fixLabels: Record<Adjustable, string> = {
  white: 'Adjust white',
  red: 'Adjust red',
  green: 'Adjust green',
  blue: 'Adjust blue',
};

const tvInstruction = 'Open Prysm on the TV and choose Calibrate.';
const needsTv = `The TV needs to show the patterns with screen sync on. ${tvInstruction}`;

/** The white controls' names: Warmth, Tint. */
const correctionLabels = Object.fromEntries(
  corrections.map(([name, label]) => [name, label]),
) as Record<keyof typeof correctionLimits, string>;

export interface CalibrateProps {
  processing: Processing;
  revision: number;
  target: LightingTarget;
  /** The pattern the TV shows, absent while not calibrating. */
  shown: CalibrationPattern | undefined;
  disabled: boolean;
  save: (expected: number, processing: Processing) => Promise<number>;
  /** Light with a processing section unsaved, while a slider is held. */
  tryProcessing: (processing: Processing) => Promise<unknown>;
  setCalibration: (pattern: CalibrationPattern | null) => Promise<unknown>;
  /** Go back to the portal's main page. */
  leave: () => void;
}

/**
 * The Calibrate page, used on a phone while the TV shows patterns: a guided
 * walk through white, red, green and blue, then checks, on a draft
 * calibration. The draft is an ordinary calibration, created and made active at the
 * start and saved live like the Picture panel; Finish names it, Cancel
 * deletes it and makes the previous calibration active again.
 */
export function CalibrateWizard({
  processing,
  revision,
  target,
  shown,
  disabled,
  save,
  tryProcessing,
  setCalibration,
  leave,
}: CalibrateProps) {
  const live = useLiveProcessing({
    processing,
    revision,
    disabled,
    save,
    tryProcessing,
  });
  const calibrations = live.value.calibrations ?? [];
  const [state, dispatch] = useReducer(wizardReducer, initialWizard);
  const { phase, busy, problem } = state;
  if (shown && !state.seen) dispatch({ type: 'patternSeen' });
  const headingRef = useRef<HTMLHeadingElement>(null);
  const sectionRef = useRef<HTMLElement>(null);
  const headingId = useId();

  const calibrating = phase.kind === 'calibrating' ? phase : null;
  const draftIndex = calibrating
    ? calibrations.findIndex(
        (calibration) => calibration.id === calibrating.draftId,
      )
    : -1;
  const draft = draftIndex >= 0 ? calibrations[draftIndex] : undefined;
  const screen = calibrating?.screen ?? 0;
  const ended = !!calibrating && state.seen && !shown && !busy;
  const locked = disabled || busy || live.saving;
  useEffect(() => {
    headingRef.current?.focus({ preventScroll: true });
    // Each screen opens at its top, where its progress and question are,
    // also after scrolling down to the last one's controls.
    if (window.scrollY > 0) window.scrollTo({ top: 0 });
  }, [phase.kind, screen, ended]);

  /**
   * Show `pattern` on the TV, then `next` as the current screen, opened from
   * the check `returnTo` if given; resolves with whether it did.
   */
  async function show(
    pattern: Pattern,
    uncorrected = false,
    next?: number,
    returnTo?: number,
  ) {
    dispatch({ type: 'requestSent' });
    try {
      await setCalibration({ pattern, uncorrected });
      dispatch({ type: 'patternShown', screen: next, returnTo });
      return true;
    } catch (error) {
      const reason = failureOf(error);
      dispatch({
        type: 'requestFailed',
        problem:
          reason.kind === 'not_processed'
            ? needsTv
            : `The TV pattern did not change: ${reason.message}`,
      });
      return false;
    }
  }
  async function start(from: string) {
    dispatch({ type: 'patternRequested' });
    if (!(await show(screens[0]!.pattern))) return;
    const source = calibrations.find((calibration) => calibration.id === from);
    const id = newCalibrationId(
      calibrations.map((calibration) => calibration.id),
    );
    const calibration = {
      id,
      name: freeName(
        source ? `${nameOf(source)} calibrated` : 'Calibrated',
        calibrations,
      ),
      correction: source ? correctionOf(source) : noCorrection,
    };
    const previous = live.value.active_calibration;
    if (
      await live.commit(
        withCalibrations(live.value, [...calibrations, calibration], id),
      )
    )
      dispatch({ type: 'started', draftId: id, previous });
  }
  async function end(title: string, message: string) {
    dispatch({ type: 'ending' });
    try {
      await setCalibration(null);
    } catch {
      // The engine ends it by itself after ten minutes.
    }
    dispatch({ type: 'ended', title, message });
  }
  function correct(change: Partial<CorrectionControls>) {
    if (!draft) return;
    const next = {
      ...draft,
      correction: { ...correctionOf(draft), ...change },
    };
    // The lights follow a held slider at once; its release saves.
    live.slider.slide(
      withCalibrations(
        live.value,
        calibrations.map((calibration, at) =>
          at === draftIndex ? next : calibration,
        ),
        live.value.active_calibration,
      ),
    );
  }
  async function finish(typedName: string | null) {
    if (!draft) return;
    const typed = (typedName ?? nameOf(draft)).trim();
    const invalid = nameProblem(typed, calibrationLimits.name);
    if (invalid) {
      // The field says what is wrong, in its own note.
      focusField(sectionRef.current, 'calibration.name');
      return;
    }
    const others = calibrations.filter(
      (calibration) => calibration.id !== draft.id,
    );
    const final = freeName(typed, others);
    if (final !== draft.name) {
      const renamed = calibrations.map((calibration) =>
        calibration.id === draft.id
          ? { ...calibration, name: final }
          : calibration,
      );
      if (
        !(await live.commit(
          withCalibrations(live.value, renamed, live.value.active_calibration),
        ))
      )
        return;
    }
    await end('Calibration finished', `“${final}” is saved and active.`);
  }
  async function cancel(draftId: string, previous: string | undefined) {
    const rest = calibrations.filter(
      (calibration) => calibration.id !== draftId,
    );
    const back = rest.some((calibration) => calibration.id === previous)
      ? previous
      : undefined;
    if (!(await live.commit(withCalibrations(live.value, rest, back)))) return;
    await end(
      'Calibration cancelled',
      'The new calibration was deleted; your colours are as before.',
    );
  }

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
      key={`${phase.kind}-${screen}-${ended}`}
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
  else if (phase.kind === 'start')
    content = (
      <StartScreen
        heading={heading}
        notices={notices}
        calibrations={calibrations}
        from={phase.from}
        choose={(from) => dispatch({ type: 'sourceChosen', from })}
        scope={live.scope}
        target={target}
        locked={locked}
        starting={busy || live.saving}
        start={() => void start(phase.from)}
        leave={leave}
      />
    );
  else
    content = (
      <StepScreen
        heading={heading}
        notices={notices}
        current={screens[phase.screen] ?? screens[0]!}
        screen={phase.screen}
        returnTo={phase.returnTo}
        draft={draft}
        path={`processing.calibrations[${draftIndex}].correction`}
        name={phase.name}
        ended={ended}
        target={target}
        uncorrected={shown?.uncorrected ?? false}
        locked={locked}
        live={live}
        dispatch={dispatch}
        correct={correct}
        show={(pattern, uncorrected, next, returnTo) =>
          void show(pattern, uncorrected, next, returnTo)
        }
        resume={() => {
          dispatch({ type: 'patternRequested' });
          void show(screens[phase.screen]!.pattern);
        }}
        finish={() => void finish(phase.name)}
        cancel={() => void cancel(phase.draftId, phase.previous)}
        leave={leave}
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
        <button type="button" className="quiet" onClick={leave}>
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
                label: nameOf(calibration),
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

/** One pattern on the TV and its controls, or the notice that calibration ended. */
function StepScreen({
  heading,
  notices,
  current,
  screen,
  returnTo,
  draft,
  path,
  name,
  ended,
  target,
  uncorrected,
  locked,
  live,
  dispatch,
  correct,
  show,
  resume,
  finish,
  cancel,
  leave,
}: {
  heading: (title: string) => ReactNode;
  notices: ReactNode;
  current: Screen;
  screen: number;
  /** The check this screen was opened from, to go back to. */
  returnTo: number | null;
  draft: Calibration | undefined;
  /** The draft's correction in the processing section. */
  path: string;
  name: string | null;
  ended: boolean;
  target: LightingTarget;
  /** The TV shows the pattern without the draft's correction. */
  uncorrected: boolean;
  locked: boolean;
  live: {
    scope: FieldScope;
    generation: number;
    /** A held slider lights its values at once and saves on release. */
    slider: { press: () => void; release: () => void };
  };
  dispatch: (action: WizardAction) => void;
  correct: (change: Partial<CorrectionControls>) => void;
  show: (
    pattern: Pattern,
    uncorrected: boolean,
    next?: number,
    returnTo?: number,
  ) => void;
  resume: () => void;
  finish: () => void;
  cancel: () => void;
  leave: () => void;
}) {
  const words = wordsOf(current);
  const fixesId = useId();
  const sectionIndex = sectionOrder.indexOf(current.section);
  const inSection = screens.filter((item) => item.section === current.section);
  const position = inSection.indexOf(current);
  const values = draft ? correctionOf(draft) : noCorrection;
  const go = (to: number, from?: number) =>
    show(screens[to]!.pattern, false, to, from);
  const slider = (control: keyof typeof whiteGuidance) => (
    <RangeField
      key={control}
      className={`slider-${control}`}
      path={`${path}.${control}`}
      label={correctionLabels[control]}
      help={whiteGuidance[control]}
      value={values[control]}
      onChange={(value) => correct({ [control]: value })}
      onPress={live.slider.press}
      onRelease={live.slider.release}
      format={signed}
      {...correctionLimits[control]}
    />
  );
  const primary = primaryOf(current.section);
  const primarySlider = (
    name: Primary,
    control: keyof typeof primaryLimits,
    ends: readonly [string, string],
  ) => (
    <RangeField
      key={control}
      className={`slider-${name}-${control}`}
      path={`${path}.${name}.${control}`}
      label={endsLabel(ends)}
      help={control === 'hue' ? hueGuidance(ends) : strengthGuidance}
      ends={ends}
      value={values[name][control]}
      onChange={(value) =>
        correct({ [name]: { ...values[name], [control]: value } })
      }
      onPress={live.slider.press}
      onRelease={live.slider.release}
      format={signed}
      {...primaryLimits[control]}
    />
  );
  const reset = resetOf(current.section, values);
  const review = current.section === 'review';
  return (
    <>
      <div className="card-heading">
        <p className="eyebrow">
          Calibrate colours
          <span className="eyebrow-step">
            {' '}
            · step {sectionIndex + 1} of {sectionOrder.length}
          </span>
        </p>
        <button
          type="button"
          className="quiet danger"
          disabled={locked}
          onClick={cancel}
        >
          Cancel calibration
        </button>
      </div>
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
      {ended ? (
        <>
          {heading('Calibration ended')}
          <p className="notice warning" role="alert">
            The TV no longer shows the patterns. Your changes so far are kept in
            “{draft ? nameOf(draft) : 'your calibration'}”.
            {target !== 'processed' ? ` ${tvInstruction}` : ''}
          </p>
          {notices}
          <div className="actions">
            <button type="button" disabled={locked} onClick={leave}>
              Leave
            </button>
            <button
              type="button"
              className="primary"
              disabled={locked}
              onClick={resume}
            >
              Resume
            </button>
          </div>
        </>
      ) : (
        <>
          {heading(words.title)}
          <p
            className="calibrate-pattern"
            data-pattern={current.pattern}
            role="status"
          >
            On your TV: {patternLabels[current.pattern]}
          </p>
          <p className="intro">{words.lead}</p>
          {draft ? (
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
                        disabled={locked}
                        onClick={() => go(screenOf(fix), screen)}
                      >
                        {fixLabels[fix]}
                      </button>
                    ))}
                  </div>
                ) : null}
                {review ? (
                  <TextField
                    path="calibration.name"
                    label="Name"
                    help="A name you will recognise, such as Movie night or PS5."
                    value={name ?? nameOf(draft)}
                    required
                    check={(value) =>
                      nameProblem(value.trim(), calibrationLimits.name)
                    }
                    onChange={(value) =>
                      dispatch({ type: 'named', name: value })
                    }
                  />
                ) : null}
              </div>
            </FieldScopeProvider>
          ) : (
            <p className="notice warning">
              This calibration was deleted elsewhere. Cancel to finish.
            </p>
          )}
          {/* Compare and Reset in one row: used often, always in the same place. */}
          <div className="calibrate-tools">
            <button
              type="button"
              className="compare"
              aria-pressed={uncorrected}
              disabled={locked}
              onClick={() => show(current.pattern, !uncorrected)}
            >
              Show without correction
            </button>
            {reset && draft ? (
              <button
                type="button"
                className="quiet"
                disabled={locked || !reset.changed}
                onClick={() => correct(reset.to)}
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
                disabled={locked}
                onClick={() => go(screen - 1)}
              >
                Back
              </button>
            ) : null}
            {review ? (
              <button
                type="button"
                className="primary"
                disabled={locked || !draft}
                onClick={finish}
              >
                Finish
              </button>
            ) : (
              <button
                type="button"
                className="primary"
                disabled={locked}
                onClick={() => go(returnTo ?? screen + 1)}
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
