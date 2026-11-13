import { useEffect, useId, useRef, useState } from 'react';
import type {
  Calibration,
  LightingTarget,
  Pattern,
} from '@prysm/application-client';
import type {
  CorrectionControls,
  HueControls,
  Processing,
} from '@prysm/contracts/application';
import { failureOf } from './adapters';
import {
  FieldScopeProvider,
  nameProblem,
  RangeField,
  SelectField,
  TextField,
} from './fields';
import { useLiveProcessing, withProfiles } from './processing';
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
} from './profiles';

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

type Section = 'white' | 'hues' | 'intensity' | 'mixed' | 'review';
const sections: Record<Section, { label: string; title: string; lead: string }> = {
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

type Hue = keyof HueControls;
interface Screen {
  section: Section;
  pattern: Pattern;
  /** The hue this screen adjusts (hue screens only). */
  hue?: Hue;
}
/** The wizard, one TV pattern per screen. */
const screens: readonly Screen[] = [
  { section: 'white', pattern: 'gray-192' },
  { section: 'hues', pattern: 'red', hue: 'red' },
  { section: 'hues', pattern: 'yellow', hue: 'yellow' },
  { section: 'hues', pattern: 'green', hue: 'green' },
  { section: 'hues', pattern: 'cyan', hue: 'cyan' },
  { section: 'hues', pattern: 'blue-pair', hue: 'blue' },
  { section: 'hues', pattern: 'magenta', hue: 'magenta' },
  { section: 'intensity', pattern: 'red-pair' },
  { section: 'intensity', pattern: 'green-pair' },
  { section: 'intensity', pattern: 'blue-pair' },
  { section: 'mixed', pattern: 'coast' },
  { section: 'mixed', pattern: 'garden' },
  { section: 'mixed', pattern: 'dusk' },
  { section: 'review', pattern: 'review-grid' },
];

const tvInstruction = 'Open Prysm on the TV and choose Calibrate.';
const needsTv = `The TV needs to show the patterns with screen sync on. ${tvInstruction}`;

const correction = Object.fromEntries(
  corrections.map(([name, label, help]) => [name, { label, help }]),
) as Record<keyof typeof correctionLimits, { label: string; help: string }>;

/** `Red hue` and which way it turns, from the hue wheel. */
function hueText(hue: Hue) {
  const at = hues.findIndex(([name]) => name === hue);
  const label = (index: number) => hues[(index + hues.length) % hues.length]![1];
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
  /** The profile being calibrated; `null` before the start. */
  const [draftId, setDraftId] = useState<string | null>(null);
  /** The active profile before the start, for Cancel. */
  const [previous, setPrevious] = useState<string | undefined>(undefined);
  const [from, setFrom] = useState('');
  const [screen, setScreen] = useState(0);
  const [name, setName] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState('');
  /** How calibration ended (finished or cancelled). */
  const [outcome, setOutcome] = useState<{ title: string; message: string } | null>(null);
  /** The TV showed a pattern since the last request, so its absence means it ended. */
  const [seen, setSeen] = useState(false);
  if (calibration && !seen) setSeen(true);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const headingId = useId();

  const draftIndex = profiles.findIndex((profile) => profile.id === draftId);
  const draft = draftIndex >= 0 ? profiles[draftIndex] : undefined;
  const current = screens[screen] ?? screens[0]!;
  const started = draftId !== null && !outcome;
  const ended = started && seen && !calibration && !busy;
  const locked = disabled || busy || live.saving;
  useEffect(() => {
    headingRef.current?.focus({ preventScroll: true });
  }, [screen, started, ended, outcome]);

  /** Show `pattern` on the TV; resolves with whether it did. */
  async function show(pattern: Pattern, uncorrected = false) {
    setBusy(true);
    setProblem('');
    try {
      await setCalibration({ pattern, uncorrected });
      return true;
    } catch (error) {
      const reason = failureOf(error);
      setProblem(
        reason.kind === 'not_processed'
          ? needsTv
          : `The TV pattern did not change: ${reason.message}`,
      );
      return false;
    } finally {
      setBusy(false);
    }
  }
  async function start() {
    setSeen(false);
    if (!(await show(screens[0]!.pattern))) return;
    const source = profiles.find((profile) => profile.id === from);
    const id = newProfileId(profiles.map((profile) => profile.id));
    const profile = {
      id,
      name: freeName(source ? `${nameOf(source)} calibrated` : 'Calibrated', profiles),
      correction: source ? correctionOf(source) : noCorrection,
    };
    const before = live.value.active_profile;
    if (await live.commit(withProfiles(live.value, [...profiles, profile], id))) {
      setPrevious(before);
      setDraftId(id);
      setScreen(0);
    }
  }
  async function go(next: number) {
    if (await show(screens[next]!.pattern)) setScreen(next);
  }
  async function resume() {
    setSeen(false);
    await show(current.pattern);
  }
  async function end(title: string, message: string) {
    setBusy(true);
    try {
      await setCalibration(null);
    } catch {
      // The engine ends it by itself after ten minutes.
    }
    setOutcome({ title, message });
    setBusy(false);
  }
  function correct(change: Partial<CorrectionControls>) {
    if (!draft) return;
    const next = { ...draft, correction: { ...correctionOf(draft), ...change } };
    live.replace(
      withProfiles(
        live.value,
        profiles.map((profile, at) => (at === draftIndex ? next : profile)),
        live.value.active_profile,
      ),
    );
  }
  async function finish() {
    if (!draft) return;
    const typed = (name ?? nameOf(draft)).trim();
    const invalid = nameProblem(typed, profileLimits.name);
    if (invalid) {
      setProblem(invalid);
      return;
    }
    const others = profiles.filter((profile) => profile.id !== draft.id);
    const final = freeName(typed, others);
    if (final !== draft.name) {
      const renamed = profiles.map((profile) =>
        profile.id === draft.id ? { ...profile, name: final } : profile,
      );
      if (!(await live.commit(withProfiles(live.value, renamed, live.value.active_profile))))
        return;
    }
    await end('Calibration finished', `“${final}” is saved and active.`);
  }
  async function cancel() {
    const rest = profiles.filter((profile) => profile.id !== draftId);
    const back = rest.some((profile) => profile.id === previous) ? previous : undefined;
    if (!(await live.commit(withProfiles(live.value, rest, back)))) return;
    await end('Calibration cancelled', 'The draft profile was deleted; your colours are as before.');
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

  if (outcome)
    return (
      <section className="card calibrate" aria-labelledby={headingId}>
        <p className="eyebrow">Calibrate colours</p>
        {heading(outcome.title)}
        <p className="intro" role="status">
          {outcome.message}
        </p>
        <div className="actions">
          <button type="button" className="primary" onClick={leave}>
            Back to Prysm
          </button>
        </div>
      </section>
    );

  if (!started) {
    const full = profiles.length >= profileLimits.count;
    return (
      <section className="card calibrate" aria-labelledby={headingId}>
        <div className="card-heading">
          <p className="eyebrow">Calibrate colours</p>
          <button type="button" className="quiet" onClick={leave}>
            Leave
          </button>
        </div>
        {heading('Match the lights to your TV')}
        <p className="intro">
          The TV shows test patterns while you adjust a colour profile here.
          Look at the screen and the wall, not at this phone.
        </p>
        <p className="help">
          Patterns are compared in the TV’s current picture mode, so calibrate
          in the mode you watch in.
        </p>
        {target !== 'processed' ? (
          <p className="notice warning" role="status">
            {needsTv}
          </p>
        ) : null}
        {full ? (
          <p className="notice warning">
            You have the most profiles Prysm keeps ({profileLimits.count}).
            Delete one to calibrate.
          </p>
        ) : null}
        <FieldScopeProvider value={live.scope}>
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
              onChange={setFrom}
            />
          ) : null}
        </FieldScopeProvider>
        {notices}
        <div className="actions">
          <button
            type="button"
            className="primary"
            disabled={locked || full || target !== 'processed'}
            onClick={() => void start()}
          >
            {busy || live.saving ? 'Starting…' : 'Start'}
          </button>
        </div>
      </section>
    );
  }

  const section = sections[current.section];
  const sectionIndex = sectionOrder.indexOf(current.section);
  const inSection = screens.filter((item) => item.section === current.section);
  const position = inSection.indexOf(current);
  const path = `processing.profiles[${draftIndex}].correction`;
  const values = draft ? correctionOf(draft) : noCorrection;
  const slider = (name: keyof typeof correctionLimits) => (
    <RangeField
      key={name}
      path={`${path}.${name}`}
      label={correction[name].label}
      help={correction[name].help}
      value={values[name]}
      onChange={(value) => correct({ [name]: value })}
      format={signed}
      {...correctionLimits[name]}
    />
  );
  const hue = current.hue ? hueText(current.hue) : null;
  return (
    <section className="card calibrate" aria-labelledby={headingId}>
      <div className="card-heading">
        <p className="eyebrow">
          Calibrate colours · step {sectionIndex + 1} of {sectionOrder.length}
        </p>
        <button
          type="button"
          className="quiet"
          disabled={locked}
          onClick={() => void cancel()}
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
              onClick={() => void resume()}
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
            {inSection.length > 1 ? ` (${position + 1} of ${inSection.length})` : ''}
          </p>
          <p className="intro">{section.lead}</p>
          {current.hue === 'blue' ? (
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
                {current.hue && hue ? (
                  <RangeField
                    path={`${path}.hue.${current.hue}`}
                    label={hue.label}
                    help={hue.help}
                    value={values.hue[current.hue]}
                    onChange={(value) =>
                      correct({ hue: { ...values.hue, [current.hue!]: value } })
                    }
                    format={signed}
                    {...hueLimits[current.hue]}
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
                    onChange={(value) => {
                      setName(value);
                      setProblem('');
                    }}
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
            aria-pressed={calibration?.uncorrected ?? false}
            disabled={locked}
            onClick={() => void show(current.pattern, !calibration?.uncorrected)}
          >
            Show without correction
          </button>
          {notices}
          <div className="actions">
            {screen > 0 ? (
              <button type="button" disabled={locked} onClick={() => void go(screen - 1)}>
                Back
              </button>
            ) : null}
            {screen < screens.length - 1 ? (
              <button
                type="button"
                className="primary"
                disabled={locked}
                onClick={() => void go(screen + 1)}
              >
                Next
              </button>
            ) : (
              <button
                type="button"
                className="primary"
                disabled={locked || !draft}
                onClick={() => void finish()}
              >
                Finish
              </button>
            )}
          </div>
        </>
      )}
    </section>
  );
}
