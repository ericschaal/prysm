import { useEffect, useId, useReducer, useRef, type ReactNode } from 'react';
import {
  failureOf,
  type Calibration,
  type LightingTarget,
  type Pattern,
} from '@prysm/application-client';
import type {
  ColorProfile,
  CorrectionControls,
  Processing,
} from '@prysm/contracts/application';
import {
  initialWizard,
  screens,
  wizardReducer,
  type Hue,
  type Screen,
  type Section,
  type WizardAction,
} from './calibrate-wizard';
import { nameProblem } from './checks';
import { RangeField, SelectField, TextField } from './fields';
import { FieldScopeProvider, type FieldScope } from './form';
import { useLiveProcessing, withProfiles } from './live-processing';
import {
  correctionLimits,
  correctionOf,
  corrections,
  freeName,
  hueLimits,
  hues,
  nameOf,
  newProfileId,
  noCorrection,
  profileLimits,
  signed,
} from './profile-model';

/** The hash route of the Calibrate page; the TV shows a QR code of it. */
export const calibrateRoute = '#/calibrate';

/** What the TV shows for each pattern. */
const patternLabels: Record<Pattern, string> = {
  'gray-192': 'light gray',
  red: 'red',
  yellow: 'yellow',
  green: 'green',
  cyan: 'cyan',
  blue: 'blue',
  magenta: 'magenta',
  'blue-pair': 'moderate blue (left), vivid blue (right)',
  'red-pair': 'moderate red (left), vivid red (right)',
  'green-pair': 'moderate green (left), vivid green (right)',
  coast: 'peach (left), teal (right)',
  garden: 'rose (left), sage (right)',
  dusk: 'amber (left), violet (right)',
  'review-grid': 'a grid of colours',
};

const sections: Record<
  Section,
  { label: string; title: string; lead: string }
> = {
  white: {
    label: 'White balance',
    title: 'Find a natural white',
    lead: 'Make the wall glow as neutral as the gray on your TV. Ignore brightness. If the wall looks blue, raise Warmth; if it looks yellow, lower it. Use Tint for a green or pink cast. Small changes are enough.',
  },
  hues: {
    label: 'Hues',
    title: 'Fine-tune each colour',
    lead: 'Change this colour only if the wall leans toward a neighbouring colour. If it differs only in brightness, leave it at 0.',
  },
  intensity: {
    label: 'Intensity',
    title: 'Match colour intensity',
    lead: 'Compare each half of the TV with the wall behind it. If the wall looks washed out, raise Intensity; if it looks too vivid, lower it. Keep a change only if it helps both halves.',
  },
  mixed: {
    label: 'Mixed colours',
    title: 'Check colours together',
    lead: 'Compare each side of the wall with the same side of the TV: the two colours should stay distinct. If one looks off, go back to its hue or to intensity.',
  },
  review: {
    label: 'Review',
    title: 'Name your profile',
    lead: 'Check the glow around the whole screen. When it looks right, name the profile and finish.',
  },
};
const sectionOrder = Object.keys(sections) as Section[];

const tvInstruction = 'Open Prysm on the TV and choose Calibrate.';
const needsTv = `The TV needs to show the patterns with screen sync on. ${tvInstruction}`;

const correction = Object.fromEntries(
  corrections.map(([name, label, help]) => [name, { label, help }]),
) as Record<keyof typeof correctionLimits, { label: string; help: string }>;

/** `Red hue` and which way it turns, from the hue wheel. */
function hueText(hue: Hue) {
  const at = hues.findIndex(([name]) => name === hue);
  const label = (index: number) =>
    hues[(index + hues.length) % hues.length]![1];
  return {
    label: `${label(at)} hue`,
    help: `Positive turns ${label(at).toLowerCase()} toward ${label(at + 1).toLowerCase()}, negative toward ${label(at - 1).toLowerCase()}.`,
  };
}

export interface CalibrateProps {
  processing: Processing;
  revision: number;
  target: LightingTarget;
  /** The pattern the TV shows, absent while not calibrating. */
  calibration: Calibration | undefined;
  disabled: boolean;
  save: (expected: number, processing: Processing) => Promise<number>;
  setCalibration: (calibration: Calibration | null) => Promise<unknown>;
  /** Go back to the portal's main page. */
  leave: () => void;
}

/**
 * The Calibrate page, used on a phone while the TV shows patterns: a guided
 * walk through white balance, hues, intensity and mixed colours on a draft
 * profile. The draft is an ordinary profile, created and made active at the
 * start and saved live like the Picture panel; Finish names it, Cancel
 * deletes it and makes the previous profile active again.
 */
export function CalibrateWizard({
  processing,
  revision,
  target,
  calibration,
  disabled,
  save,
  setCalibration,
  leave,
}: CalibrateProps) {
  const live = useLiveProcessing({ processing, revision, disabled, save });
  const profiles = live.value.profiles ?? [];
  const [state, dispatch] = useReducer(wizardReducer, initialWizard);
  const { phase, busy, problem } = state;
  if (calibration && !state.seen) dispatch({ type: 'patternSeen' });
  const headingRef = useRef<HTMLHeadingElement>(null);
  const headingId = useId();

  const calibrating = phase.kind === 'calibrating' ? phase : null;
  const draftIndex = calibrating
    ? profiles.findIndex((profile) => profile.id === calibrating.draftId)
    : -1;
  const draft = draftIndex >= 0 ? profiles[draftIndex] : undefined;
  const screen = calibrating?.screen ?? 0;
  const ended = !!calibrating && state.seen && !calibration && !busy;
  const locked = disabled || busy || live.saving;
  useEffect(() => {
    headingRef.current?.focus({ preventScroll: true });
  }, [phase.kind, screen, ended]);

  /** Show `pattern` on the TV, then `next` as the current screen; resolves with whether it did. */
  async function show(pattern: Pattern, uncorrected = false, next?: number) {
    dispatch({ type: 'requestSent' });
    try {
      await setCalibration({ pattern, uncorrected });
      dispatch({ type: 'patternShown', screen: next });
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
    const source = profiles.find((profile) => profile.id === from);
    const id = newProfileId(profiles.map((profile) => profile.id));
    const profile = {
      id,
      name: freeName(
        source ? `${nameOf(source)} calibrated` : 'Calibrated',
        profiles,
      ),
      correction: source ? correctionOf(source) : noCorrection,
    };
    const previous = live.value.active_profile;
    if (await live.commit(withProfiles(live.value, [...profiles, profile], id)))
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
    live.replace(
      withProfiles(
        live.value,
        profiles.map((profile, at) => (at === draftIndex ? next : profile)),
        live.value.active_profile,
      ),
    );
  }
  async function finish(typedName: string | null) {
    if (!draft) return;
    const typed = (typedName ?? nameOf(draft)).trim();
    const invalid = nameProblem(typed, profileLimits.name);
    if (invalid) {
      dispatch({ type: 'refused', problem: invalid });
      return;
    }
    const others = profiles.filter((profile) => profile.id !== draft.id);
    const final = freeName(typed, others);
    if (final !== draft.name) {
      const renamed = profiles.map((profile) =>
        profile.id === draft.id ? { ...profile, name: final } : profile,
      );
      if (
        !(await live.commit(
          withProfiles(live.value, renamed, live.value.active_profile),
        ))
      )
        return;
    }
    await end('Calibration finished', `“${final}” is saved and active.`);
  }
  async function cancel(draftId: string, previous: string | undefined) {
    const rest = profiles.filter((profile) => profile.id !== draftId);
    const back = rest.some((profile) => profile.id === previous)
      ? previous
      : undefined;
    if (!(await live.commit(withProfiles(live.value, rest, back)))) return;
    await end(
      'Calibration cancelled',
      'The draft profile was deleted; your colours are as before.',
    );
  }

  const notices = (
    <>
      {problem ? (
        <p className="notice warning" role="alert">
          {problem}
        </p>
      ) : null}
      {live.notice ? (
        <p className="notice warning" role="status">
          {live.notice}
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
    <h2 id={headingId} ref={headingRef} tabIndex={-1}>
      {title}
    </h2>
  );

  let content: ReactNode;
  if (phase.kind === 'done')
    content = (
      <>
        <p className="eyebrow">Calibrate colours</p>
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
        profiles={profiles}
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
        draft={draft}
        path={`processing.profiles[${draftIndex}].correction`}
        name={phase.name}
        ended={ended}
        target={target}
        uncorrected={calibration?.uncorrected ?? false}
        locked={locked}
        live={live}
        dispatch={dispatch}
        correct={correct}
        show={(pattern, uncorrected, next) =>
          void show(pattern, uncorrected, next)
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
    <section className="card calibrate" aria-labelledby={headingId}>
      {content}
    </section>
  );
}

/** Before the start: what to copy, and why the start may not work yet. */
function StartScreen({
  heading,
  notices,
  profiles,
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
  profiles: readonly ColorProfile[];
  from: string;
  choose: (from: string) => void;
  scope: FieldScope;
  target: LightingTarget;
  locked: boolean;
  starting: boolean;
  start: () => void;
  leave: () => void;
}) {
  const full = profiles.length >= profileLimits.count;
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
        The TV shows test patterns while you adjust a colour profile here. Look
        at the screen and the wall, not at this phone.
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
          You have the most profiles Prysm keeps ({profileLimits.count}). Delete
          one to calibrate.
        </p>
      ) : null}
      <FieldScopeProvider value={scope}>
        {profiles.length ? (
          <SelectField
            path="calibration.from"
            label="Start from"
            help="Calibration works on a copy; the original profile stays as it is."
            value={from}
            options={[
              { value: '', label: 'New profile' },
              ...profiles.map((profile) => ({
                value: profile.id,
                label: nameOf(profile),
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
  draft: ColorProfile | undefined;
  /** The draft's correction in the processing section. */
  path: string;
  name: string | null;
  ended: boolean;
  target: LightingTarget;
  /** The TV shows the pattern without the draft's correction. */
  uncorrected: boolean;
  locked: boolean;
  live: { scope: FieldScope; generation: number };
  dispatch: (action: WizardAction) => void;
  correct: (change: Partial<CorrectionControls>) => void;
  show: (pattern: Pattern, uncorrected: boolean, next?: number) => void;
  resume: () => void;
  finish: () => void;
  cancel: () => void;
  leave: () => void;
}) {
  const section = sections[current.section];
  const sectionIndex = sectionOrder.indexOf(current.section);
  const inSection = screens.filter((item) => item.section === current.section);
  const position = inSection.indexOf(current);
  const values = draft ? correctionOf(draft) : noCorrection;
  const slider = (control: keyof typeof correctionLimits) => (
    <RangeField
      key={control}
      path={`${path}.${control}`}
      label={correction[control].label}
      help={correction[control].help}
      value={values[control]}
      onChange={(value) => correct({ [control]: value })}
      format={signed}
      {...correctionLimits[control]}
    />
  );
  const hue = current.hue;
  return (
    <>
      <div className="card-heading">
        <p className="eyebrow">
          Calibrate colours · step {sectionIndex + 1} of {sectionOrder.length}
        </p>
        <button
          type="button"
          className="quiet"
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
            {sections[id].label}
          </li>
        ))}
      </ol>
      {ended ? (
        <>
          {heading('Calibration ended')}
          <p className="notice warning" role="alert">
            The TV no longer shows the patterns. Your changes so far are kept in
            “{draft ? nameOf(draft) : 'your profile'}”.
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
          {heading(section.title)}
          <p className="calibrate-pattern" role="status">
            On your TV: {patternLabels[current.pattern]}
            {inSection.length > 1
              ? ` (${position + 1} of ${inSection.length})`
              : ''}
          </p>
          <p className="intro">{section.lead}</p>
          {hue === 'blue' ? (
            <p className="help">
              If the wall blue looks purple, move toward cyan; if it looks teal,
              toward magenta.
            </p>
          ) : null}
          {draft ? (
            <FieldScopeProvider value={live.scope}>
              <div key={live.generation} className="calibrate-controls">
                {current.section === 'white' ? (
                  <>
                    {slider('warmth')}
                    {slider('tint')}
                  </>
                ) : null}
                {hue ? (
                  <RangeField
                    path={`${path}.hue.${hue}`}
                    {...hueText(hue)}
                    value={values.hue[hue]}
                    onChange={(value) =>
                      correct({ hue: { ...values.hue, [hue]: value } })
                    }
                    format={signed}
                    {...hueLimits[hue]}
                  />
                ) : null}
                {current.section === 'intensity' ? slider('intensity') : null}
                {current.section === 'review' ? (
                  <TextField
                    path="calibration.name"
                    label="Profile name"
                    help="A name you will recognise, such as Movie night or PS5."
                    value={name ?? nameOf(draft)}
                    required
                    onChange={(value) =>
                      dispatch({ type: 'named', name: value })
                    }
                  />
                ) : null}
              </div>
            </FieldScopeProvider>
          ) : (
            <p className="notice warning">
              The profile being calibrated was deleted elsewhere. Cancel to
              finish.
            </p>
          )}
          <button
            type="button"
            className="compare"
            aria-pressed={uncorrected}
            disabled={locked}
            onClick={() => show(current.pattern, !uncorrected)}
          >
            Show without correction
          </button>
          {notices}
          <div className="actions">
            {screen > 0 ? (
              <button
                type="button"
                disabled={locked}
                onClick={() =>
                  show(screens[screen - 1]!.pattern, false, screen - 1)
                }
              >
                Back
              </button>
            ) : null}
            {screen < screens.length - 1 ? (
              <button
                type="button"
                className="primary"
                disabled={locked}
                onClick={() =>
                  show(screens[screen + 1]!.pattern, false, screen + 1)
                }
              >
                Next
              </button>
            ) : (
              <button
                type="button"
                className="primary"
                disabled={locked || !draft}
                onClick={finish}
              >
                Finish
              </button>
            )}
          </div>
        </>
      )}
    </>
  );
}
