import { useEffect, useRef, useState } from 'react';
import { useCalibrationSession } from './useCalibrationSession';
import {
  CalibrationSlider,
  WhiteBalanceControls,
  SwatchPicker,
} from './CalibrationControls';
import { Link } from 'react-router';
import { useControl } from './useControl';
import patterns from '../../calibration-patterns.json';
import type { components } from './api.generated';

const { colors } = patterns;
function splitSwatches(patches: typeof patterns.pairs) {
  return patches.map((pair) => ({
    ...pair,
    swatch: `linear-gradient(90deg,${pair.leftColor} 50%,${pair.rightColor} 50%)`,
  }));
}
const intensity = splitSwatches(patterns.intensity);
const pairs = splitSwatches(patterns.pairs);
type View = components['schemas']['CalibrationView'];
type Step = View['step'];
const steps: { id: Step; label: string; title: string }[] = [
  { id: 'choose', label: 'Profile', title: 'Choose a profile' },
  { id: 'response', label: 'Response', title: 'Check the response' },
  {
    id: 'white_balance',
    label: 'White balance',
    title: 'Find a natural white',
  },
  { id: 'intensity', label: 'Intensity', title: 'Match color intensity' },
  { id: 'colors', label: 'Hues', title: 'Fine-tune individual hues' },
  { id: 'mixed', label: 'Mixed colors', title: 'Check colors together' },
  { id: 'review', label: 'Save', title: 'Make it yours' },
];
const neutral: View['draft']['correction'] = {
  warmth: 0,
  tint: 0,
  midtones: 0,
};
const wheel = ['Red', 'Yellow', 'Green', 'Cyan', 'Blue', 'Magenta'];
export function Calibration({ pairing }: { pairing: string }) {
  const control = useControl();
  const { settings } = control;
  const {
    view: session,
    error,
    busy,
    saving,
    canCancel,
    rejected,
    update,
    comparison,
    selectProfile,
    save,
    cancel,
  } = useCalibrationSession(pairing);
  const [editingHues, setEditingHues] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [editedName, setEditedName] = useState<string | null>(null);
  const view = settings ? session : null;
  const profiles = settings?.settings.calibration.profiles ?? [];
  const stopError =
    control.requestError ||
    (control.pendingControl ? control.connectionError : '');
  const atCapacity = profiles.length >= 32;
  const stopControl = settings && (
    <>
      {stopError && <p role="alert">{stopError}</p>}
      <button
        className="calibration-text-button danger"
        disabled={control.busy}
        onClick={() => {
          comparison(false);
          void control.setState(
            {
              power: false,
              source: control.data?.source ?? settings.settings.source,
            },
            'power',
          );
        }}
      >
        {control.busy ? 'Stopping…' : 'Stop lighting'}
      </button>
    </>
  );
  const name = editedName ?? view?.draft.name ?? '';
  function select(profile: string | null) {
    setConfirmed(false);
    setEditedName(null);
    selectProfile(profile);
  }
  function move(step: Step) {
    if (step === 'colors') setEditingHues(false);
    comparison(false);
    update({
      step,
      paused: step === 'colors',
      shade:
        step === 'colors' ? colors.findIndex(({ clip }) => clip === 'blue') : 0,
    });
  }
  const correction = view?.draft.correction ?? neutral;
  const headingRef = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    headingRef.current?.focus({ preventScroll: true });
  }, [view?.step, session?.ended, rejected]);
  const tvColor = colors[view?.shade ?? 0];
  const hueIndex = wheel.indexOf(tvColor?.label ?? 'Red');
  const hue = correction.hue ?? [0, 0, 0, 0, 0, 0];
  function changeCorrection(patch: Partial<View['draft']['correction']>) {
    update({ correction: { ...correction, ...patch } });
  }
  function changeHue(value: number) {
    setEditingHues(true);
    changeCorrection({ hue: hue.map((v, i) => (i === hueIndex ? value : v)) });
  }
  const whiteBalance = (
    <WhiteBalanceControls value={correction} onChange={changeCorrection} />
  );
  const stepIndex = steps.findIndex(
    ({ id }) => id === (view?.step ?? 'choose'),
  );
  const previous = steps[stepIndex - 1]?.id;
  const next = steps[stepIndex + 1]?.id;
  const title = view ? steps[stepIndex]?.title : 'Connecting to your TV…';
  const pair = (view?.step === 'intensity' ? intensity : pairs)[
    view?.shade ?? 0
  ];

  if (session?.ended || rejected)
    return (
      <section className="calibration-wizard calibration-complete">
        <span className="completion-mark" aria-hidden="true">
          {session?.saved ? '✓' : '—'}
        </span>
        <p className="eyebrow">
          {session?.saved ? 'READY FOR YOUR NEXT SCREEN SYNC' : 'SESSION ENDED'}
        </p>
        <h1 ref={headingRef} tabIndex={-1}>
          {session?.saved ? 'Your light. Your color.' : 'Calibration ended'}
        </h1>
        <p role={rejected ? 'alert' : 'status'}>
          {rejected ? error : session?.ended}
        </p>
        {rejected && (
          <p>Start Calibrate Colors in the Prysm TV app to get a fresh link.</p>
        )}
        {session?.saved ? (
          <>
            <p className="saved-profile">{session.draft.name}</p>
            <p>Press Return on your TV remote to exit calibration.</p>
          </>
        ) : null}
        <Link className="calibration-return" to="/overview">
          Return to Overview
        </Link>
        {stopControl}
      </section>
    );
  return (
    <section className="calibration-wizard" aria-label="Calibrate Colors">
      <div className="calibration-topline">
        <p className="eyebrow">CALIBRATE COLORS</p>
        <span>
          {view ? `${stepIndex + 1} / ${steps.length}` : 'CONNECTING'}
        </span>
      </div>
      <ol className="calibration-progress" aria-label="Calibration progress">
        {steps.map(({ label }, i) => (
          <li
            key={label}
            aria-current={i === stepIndex ? 'step' : undefined}
            data-complete={i < stepIndex}
          >
            <span>{label}</span>
          </li>
        ))}
      </ol>
      <h1 ref={headingRef} tabIndex={-1}>
        {title}
      </h1>
      {error ? (
        <p className="notice error" role="alert">
          {error}
        </p>
      ) : null}
      {view ? (
        <>
          <div className="calibration-body">
            {view.step === 'choose' ? (
              <>
                <p className="calibration-lead">
                  A few small adjustments for a wall glow that feels closer to
                  your screen.
                </p>
                <div className="calibration-note">
                  <strong>Your TV is the reference</strong>
                  <p>
                    Look at the screen and the wall as you adjust. The colors on
                    this phone are controls, not a reference.
                  </p>
                </div>
                <button
                  className="primary calibration-new"
                  disabled={busy || atCapacity}
                  onClick={() => select(null)}
                >
                  New profile
                </button>
                {atCapacity && (
                  <p>
                    All 32 profile slots are in use. Adjust a saved profile, or
                    exit and delete one in Settings.
                  </p>
                )}
                {profiles.length > 0 ? (
                  <label className="calibration-field">
                    Or adjust a saved profile
                    <select
                      defaultValue=""
                      disabled={busy}
                      onChange={(e) => {
                        if (e.target.value) select(e.target.value);
                      }}
                    >
                      <option value="">Choose a profile</option>
                      {profiles.map((profile) => (
                        <option key={profile.id} value={profile.id}>
                          {profile.name}
                        </option>
                      ))}
                    </select>
                  </label>
                ) : null}
                <p className="calibration-hint">
                  Changes stay temporary until you save.
                </p>
              </>
            ) : null}
            {view.step === 'response' ? (
              <>
                <p className="calibration-lead">
                  Watch your TV. The wall lights should follow the gray, red,
                  green, and blue sequence.
                </p>
                <div className="response-swatches" aria-hidden="true">
                  <i />
                  <i />
                  <i />
                  <i />
                </div>
                <label className="calibration-confirm">
                  <input
                    type="checkbox"
                    checked={confirmed}
                    onChange={(e) => setConfirmed(e.target.checked)}
                  />
                  <span>
                    I can see the pattern and the lights are responding
                  </span>
                </label>
                <details className="calibration-details">
                  <summary>The lights are not responding</summary>
                  <p>
                    Check that WLED is powered and connected, Prysm uses the
                    webOS input, and the TV sender says Capturing. If the video
                    is missing, exit on the TV and try again.
                  </p>
                </details>
              </>
            ) : null}
            {view.step === 'white_balance' ? (
              <>
                <p className="calibration-lead">
                  Look for a color cast in the wall glow. Make it feel as
                  neutral as the gray on your TV.
                </p>
                <p className="calibration-tip">
                  Match the tint. The wall does not need to be as bright as the
                  screen.
                </p>
                <div className="calibration-adjustments">{whiteBalance}</div>
                {correction.midtones !== 0 && (
                  <button
                    className="calibration-text-button"
                    onClick={() => changeCorrection({ midtones: 0 })}
                  >
                    Reset saved dark-tone adjustment ({correction.midtones})
                  </button>
                )}
                <details className="calibration-details">
                  <summary>What should I look for?</summary>
                  <p>
                    If the wall looks blue, move toward Warmer. If it looks
                    yellow, move toward Cooler. Use Tint for a green or pink
                    cast. Small changes are enough; leave a slider at zero if
                    you’re unsure.
                  </p>
                </details>
              </>
            ) : null}
            {view.step === 'intensity' || view.step === 'mixed' ? (
              <>
                <p className="calibration-lead">
                  {view.step === 'intensity'
                    ? 'Compare each side of the TV with the wall behind it. If the wall looks washed out, raise intensity. If it looks too vivid, lower it. Keep the blues distinct, then check red and green.'
                    : 'Compare the left and right sides of the wall with the same sides of the TV. Look for the two colors to stay distinct with your everyday settings.'}
                </p>
                <SwatchPicker
                  label="TV pattern"
                  options={view.step === 'intensity' ? intensity : pairs}
                  selected={view.shade}
                  onSelect={(shade) => update({ shade })}
                />
                <p className="calibration-pattern" role="status">
                  {pair?.left} on the left · {pair?.right} on the right
                </p>
                {view.step === 'intensity' ? (
                  <>
                    <CalibrationSlider
                      field="intensity"
                      label="Color intensity"
                      low="More muted"
                      high="More vivid"
                      value={correction.intensity ?? 0}
                      onChange={(intensity) => changeCorrection({ intensity })}
                      resetLabel="Reset intensity"
                    />
                    <p className="calibration-tip">
                      If neither setting looks closer on both sides, leave
                      intensity as it was.
                    </p>
                  </>
                ) : (
                  <>
                    <p className="calibration-tip">
                      Hold for uncorrected to compare. If one color looks off,
                      use Back to revisit its hue. If all colors feel too strong
                      or weak, revisit intensity.
                    </p>
                    <button
                      className="calibration-text-button"
                      onClick={() => move('intensity')}
                    >
                      Revisit color intensity
                    </button>
                  </>
                )}
              </>
            ) : null}
            {view.step === 'colors' ? (
              <>
                <p className="calibration-lead">
                  Choose a color only if it leans toward the wrong neighbor. If
                  the difference is brightness alone, leave its hue unchanged.
                </p>
                <SwatchPicker
                  label="TV color"
                  options={colors}
                  selected={view.paused ? view.shade : null}
                  onSelect={(shade) => {
                    setEditingHues(true);
                    update({ shade, paused: true });
                  }}
                />
                <p className="calibration-pattern" role="status">
                  {view.paused
                    ? tvColor?.label + ' on your TV'
                    : 'Cycling through all six colors'}
                  {view.paused &&
                    tvColor?.clip === 'blue' &&
                    ' · Moderate blue on the left · Vivid blue on the right. Does the wall blue lean too purple or too teal?'}
                </p>
                <button
                  className="calibration-text-button"
                  aria-pressed={!view.paused}
                  onClick={() => update({ paused: !view.paused })}
                >
                  {view.paused ? 'Cycle colors' : 'Use static colors'}
                </button>
                {view.paused ? (
                  <div className="calibration-adjustments">
                    <CalibrationSlider
                      field="hue"
                      label={wheel[hueIndex] + ' hue'}
                      low={'Toward ' + wheel[(hueIndex + 5) % 6]}
                      high={'Toward ' + wheel[(hueIndex + 1) % 6]}
                      value={hue[hueIndex] ?? 0}
                      onChange={changeHue}
                      resetLabel={`Reset ${wheel[hueIndex]?.toLowerCase()} hue`}
                    />
                  </div>
                ) : null}
                <details className="calibration-details">
                  <summary>Refine the overall color balance</summary>
                  <p>
                    These controls affect every color. Recheck the other colors
                    after a change. Some differences come from the LEDs and wall
                    and cannot be removed by white balance.
                  </p>
                  {whiteBalance}
                </details>
              </>
            ) : null}
            {view.step === 'review' ? (
              <>
                <p className="calibration-lead">
                  Check the glow at each corner. When it looks right, name and
                  save your profile.
                </p>
                <div className="calibration-note">
                  <strong>Your everyday settings are back</strong>
                  <p>
                    Brightness, smoothing, and shadow lift are restored. Your
                    color adjustments are still applied.
                  </p>
                </div>
                <label className="calibration-field">
                  Profile name
                  <input
                    value={name}
                    maxLength={64}
                    autoComplete="off"
                    onChange={(e) => setEditedName(e.target.value)}
                  />
                </label>
                <p className="calibration-hint">
                  Choose a name you’ll recognize, like Movie night or PS5.
                  Saving selects this profile for your lights.
                </p>
              </>
            ) : null}
          </div>
          {view.step !== 'choose' && view.step !== 'response' ? (
            <button
              className="compare-button"
              aria-pressed={view.compare}
              onPointerDown={(e) => {
                e.currentTarget.setPointerCapture(e.pointerId);
                comparison(true);
              }}
              onPointerUp={() => comparison(false)}
              onPointerCancel={() => comparison(false)}
              onLostPointerCapture={() => comparison(false)}
              onBlur={() => comparison(false)}
              onKeyDown={(e) => {
                if ((e.key === ' ' || e.key === 'Enter') && !e.repeat) {
                  e.preventDefault();
                  comparison(true);
                }
              }}
              onKeyUp={(e) => {
                if (e.key === ' ' || e.key === 'Enter') {
                  e.preventDefault();
                  comparison(false);
                }
              }}
            >
              Hold for uncorrected
            </button>
          ) : null}
          <div className="calibration-actions">
            {previous ? (
              <button disabled={busy} onClick={() => move(previous)}>
                Back
              </button>
            ) : null}
            {view.step === 'review' && (
              <button
                className="primary"
                disabled={busy || !name.trim()}
                onClick={() => save(name.trim())}
              >
                {saving ? 'Saving…' : 'Save and use'}
              </button>
            )}
            {next && view.step !== 'choose' && (
              <button
                className="primary"
                disabled={busy || (view.step === 'response' && !confirmed)}
                onClick={() => move(next)}
              >
                {view.step === 'colors' && !editingHues
                  ? 'Skip hue adjustment'
                  : 'Next'}
              </button>
            )}
          </div>
        </>
      ) : (
        <>
          <p className="calibration-lead" role="status">
            {settings
              ? 'Keep the Prysm app open on your TV.'
              : control.connectionError || 'Getting your profiles ready.'}
          </p>
          {!settings && control.connectionError && (
            <button onClick={() => void control.poll()}>Try again</button>
          )}
        </>
      )}
      <footer className="calibration-footer">
        {view ? (
          <button
            className="calibration-text-button"
            disabled={!canCancel}
            onClick={cancel}
          >
            Cancel
          </button>
        ) : null}
        {stopControl}
      </footer>
    </section>
  );
}
