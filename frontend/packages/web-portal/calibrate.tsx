import {
  useEffect,
  useEffectEvent,
  useId,
  useReducer,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import {
  failureOf,
  same,
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
  type Place,
  type Screen,
  type Section,
  type WizardAction,
} from './calibrate-wizard';
import { nameProblem } from './checks';
import { RangeField, SelectField, TextField } from './fields';
import { FieldScopeProvider, focusField, type FieldScope } from './form';
import {
  throttled,
  useLiveProcessing,
  withCalibrations,
} from './live-processing';
import {
  correctionLimits,
  correctionOf,
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
/** White's sliders, named by their ends like the colours': warmth adds red and takes blue, tint takes green. */
const whiteEnds = {
  warmth: ['Cooler', 'Warmer'],
  tint: ['More green', 'More pink'],
} as const;
const whiteGuidance = {
  warmth: 'Too blue? Toward warmer. Too yellow? Toward cooler.',
  tint: hueGuidance(whiteEnds.tint),
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

/** Why the TV did not show a pattern. */
function tvProblem(error: unknown) {
  const reason = failureOf(error);
  return reason.kind === 'not_processed'
    ? needsTv
    : `The TV pattern did not change: ${reason.message}`;
}

/** A pattern asked of the TV, and the wizard's place with it. */
interface Wanted {
  pattern: Pattern;
  place: Place;
}

/** What waits for the save on its way: the last change must be in first. */
type Closing = 'finish' | 'discard' | 'leave';

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
  /** The TV's patterns: one request at a time, the newest waiting one next. */
  const [patterns] = useState(() => throttled<Wanted>(0));
  /** The pattern asked for last; an older one's answer no longer decides. */
  const newestRef = useRef<Wanted | null>(null);
  /** The draft's correction at the start: unchanged, Cancel needs no question. */
  const startedWithRef = useRef<CorrectionControls | null>(null);
  /** Cancel asks before discarding changes. */
  const [confirming, setConfirming] = useState(false);
  /** Finish, Cancel or Leave was tapped and is under way. */
  const [closing, setClosing] = useState<Closing | null>(null);

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
   * Go to `to` (the current screen when absent), opened from the check
   * `returnTo`, at once; the TV shows `pattern` after the request on its way.
   */
  function request(
    pattern: Pattern,
    to: { screen?: number; returnTo?: number; uncorrected?: boolean } = {},
  ) {
    const moved = to.screen !== undefined;
    const wanted: Wanted = {
      pattern,
      place: {
        screen: to.screen ?? screen,
        returnTo: moved
          ? (to.returnTo ?? null)
          : (calibrating?.returnTo ?? null),
        uncorrected: to.uncorrected ?? false,
      },
    };
    newestRef.current = wanted;
    dispatch({ type: 'requestSent', ...to });
    patterns.push(wanted, send);
  }
  async function send(wanted: Wanted) {
    try {
      await setCalibration({
        pattern: wanted.pattern,
        uncorrected: wanted.place.uncorrected,
      });
      dispatch(
        newestRef.current === wanted
          ? { type: 'patternShown' }
          : { type: 'patternShown', more: wanted.place },
      );
    } catch (error) {
      // A newer request decides what the TV shows.
      if (newestRef.current === wanted)
        dispatch({ type: 'requestFailed', problem: tvProblem(error) });
    }
  }
  async function start(from: string) {
    dispatch({ type: 'patternRequested' });
    dispatch({ type: 'requestSent' });
    try {
      await setCalibration({
        pattern: screens[0]!.pattern,
        uncorrected: false,
      });
    } catch (error) {
      dispatch({ type: 'requestFailed', problem: tvProblem(error) });
      return;
    }
    dispatch({ type: 'patternShown' });
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
    ) {
      startedWithRef.current = calibration.correction;
      dispatch({ type: 'started', draftId: id, previous });
    }
  }
  async function end(title: string, message: string) {
    dispatch({ type: 'ending' });
    // A pattern still on its way must not show after the end.
    newestRef.current = null;
    await patterns.settle();
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

  /** Save a change still waiting for its delay, then leave. */
  async function flush() {
    if (live.dirty && !(await live.commit(live.value))) return false;
    leave();
    return true;
  }
  function finish() {
    if (!draft) return;
    const typed = (calibrating?.name ?? nameOf(draft)).trim();
    if (nameProblem(typed, calibrationLimits.name)) {
      // The field says what is wrong, in its own note.
      focusField(sectionRef.current, 'calibration.name');
      return;
    }
    setClosing('finish');
  }
  async function complete() {
    if (!draft) return false;
    const typed = (calibrating?.name ?? nameOf(draft)).trim();
    const others = calibrations.filter(
      (calibration) => calibration.id !== draft.id,
    );
    const final = freeName(typed, others);
    if (final !== draft.name || live.dirty) {
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
        return false;
    }
    await end('Calibration finished', `“${final}” is saved and active.`);
    return true;
  }
  function cancel() {
    // Something would be lost: a slider moved or a name typed since the start.
    const adjusted =
      !!draft &&
      ((startedWithRef.current !== null &&
        !same(correctionOf(draft), startedWithRef.current)) ||
        (calibrating?.name != null &&
          calibrating.name.trim() !== nameOf(draft)));
    if (adjusted) setConfirming(true);
    else setClosing('discard');
  }
  async function discard() {
    if (!calibrating) return false;
    const rest = calibrations.filter(
      (calibration) => calibration.id !== calibrating.draftId,
    );
    const back = rest.some(
      (calibration) => calibration.id === calibrating.previous,
    )
      ? calibrating.previous
      : undefined;
    if (!(await live.commit(withCalibrations(live.value, rest, back))))
      return false;
    await end(
      'Calibration cancelled',
      'The new calibration was deleted; your colours are as before.',
    );
    return true;
  }

  /**
   * Finish, Cancel and Leave run once no save is on its way: a save asked
   * for meanwhile would only wait for that one, so the tap waits here
   * instead and runs with the settings that save returned.
   */
  const ready = closing !== null && !live.saving;
  const runningRef = useRef(false);
  const runClosing = useEffectEvent((how: Closing) => {
    if (runningRef.current) return;
    runningRef.current = true;
    let run = flush;
    if (how === 'finish') run = complete;
    else if (how === 'discard') run = discard;
    void run().then((done) => {
      runningRef.current = false;
      // Not done (a save failed and says why): the buttons work again.
      if (!done) setClosing(null);
    });
  });
  useEffect(() => {
    if (ready && closing) runClosing(closing);
  }, [ready, closing]);

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
        pending={busy}
        uncorrected={state.uncorrected}
        disabled={disabled}
        closing={closing}
        live={live}
        dispatch={dispatch}
        correct={correct}
        request={request}
        resume={() => {
          dispatch({ type: 'patternRequested' });
          request(screens[phase.screen]!.pattern);
        }}
        finish={finish}
        cancel={cancel}
        confirming={confirming}
        keep={() => setConfirming(false)}
        discard={() => setClosing('discard')}
        leave={() => setClosing('leave')}
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
  pending,
  uncorrected,
  disabled,
  closing,
  live,
  dispatch,
  correct,
  request,
  resume,
  finish,
  cancel,
  confirming,
  keep,
  discard,
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
  /** The pattern is on its way to the TV. */
  pending: boolean;
  /** The TV shows (or is about to show) the pattern without the draft's correction. */
  uncorrected: boolean;
  /** The portal cannot take commands now. */
  disabled: boolean;
  /** Finish, Cancel or Leave is under way. */
  closing: Closing | null;
  live: {
    scope: FieldScope;
    generation: number;
    /** A held slider lights its values at once and saves on release. */
    slider: { press: () => void; release: () => void };
  };
  dispatch: (action: WizardAction) => void;
  correct: (change: Partial<CorrectionControls>) => void;
  /** Go there at once; the TV's pattern follows. */
  request: (
    pattern: Pattern,
    to?: { screen?: number; returnTo?: number; uncorrected?: boolean },
  ) => void;
  resume: () => void;
  finish: () => void;
  cancel: () => void;
  confirming: boolean;
  keep: () => void;
  discard: () => void;
  leave: () => void;
}) {
  const words = wordsOf(current);
  const fixesId = useId();
  const sectionIndex = sectionOrder.indexOf(current.section);
  const inSection = screens.filter((item) => item.section === current.section);
  const position = inSection.indexOf(current);
  const values = draft ? correctionOf(draft) : noCorrection;
  const go = (to: number, from?: number) =>
    request(screens[to]!.pattern, { screen: to, returnTo: from });
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
      (value) => correct({ [control]: value }),
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
      (value) => correct({ [name]: { ...values[name], [control]: value } }),
      primaryLimits[control],
    );
  const reset = resetOf(current.section, values);
  const review = current.section === 'review';
  return (
    <>
      <StepHeading
        confirming={confirming}
        closing={closing}
        disabled={disabled}
        cancel={cancel}
        keep={keep}
        discard={discard}
      />
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
            <button
              type="button"
              disabled={closing !== null}
              aria-busy={closing === 'leave' || undefined}
              onClick={leave}
            >
              Leave
            </button>
            <button
              type="button"
              className="primary"
              disabled={disabled || closing !== null}
              onClick={resume}
            >
              Resume
            </button>
          </div>
        </>
      ) : (
        <>
          {heading(words.title)}
          {/* While the TV is still changing, the swatch waits dimmed. */}
          <p
            className="calibrate-pattern"
            data-pattern={current.pattern}
            role="status"
            aria-busy={pending || undefined}
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
                        disabled={disabled}
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
              disabled={disabled}
              onClick={() =>
                request(current.pattern, { uncorrected: !uncorrected })
              }
            >
              Show without correction
            </button>
            {reset && draft ? (
              <button
                type="button"
                className="quiet"
                disabled={disabled || !reset.changed}
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
                disabled={disabled}
                onClick={() => go(screen - 1)}
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
