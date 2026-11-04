import { useEffect, useRef, useState } from 'react';
import { useCalibrationSession } from './useCalibrationSession';
import { Link } from 'react-router';
import type { SettingsView } from './api';
import type { components } from './api.generated';
type View = components['schemas']['CalibrationView'];
type Step = View['step'];
const steps: Step[] = [
  'choose',
  'response',
  'white_balance',
  'intensity',
  'colors',
  'mixed',
  'review',
];
const titles = {
  choose: 'Choose a profile',
  response: 'Check the response',
  white_balance: 'Find a natural white',
  grays: 'Fine-tune dark tones',
  intensity: 'Match color intensity',
  colors: 'Fine-tune individual hues',
  mixed: 'Check colors together',
  review: 'Make it yours',
};
const neutral: View['draft']['correction'] = {
  warmth: 0,
  tint: 0,
  midtones: 0,
};
const wheel = ['Red', 'Yellow', 'Green', 'Cyan', 'Blue', 'Magenta'];
const colorNames = ['Red', 'Green', 'Blue', 'Cyan', 'Magenta', 'Yellow'];
const muted = [
  { label: 'Clay', swatch: '#c4765f' },
  { label: 'Leaf', swatch: '#78a666' },
  { label: 'Denim', swatch: '#6d90bd' },
];
const pairs = [
  {
    label: 'Coast',
    left: 'Peach',
    right: 'Teal',
    swatch: 'linear-gradient(90deg,#e7ad8a 50%,#419ea5 50%)',
  },
  {
    label: 'Garden',
    left: 'Rose',
    right: 'Sage',
    swatch: 'linear-gradient(90deg,#c98b9d 50%,#859b6c 50%)',
  },
  {
    label: 'Dusk',
    left: 'Amber',
    right: 'Violet',
    swatch: 'linear-gradient(90deg,#d59d4a 50%,#8165a9 50%)',
  },
];

export function Calibration({
  pairing,
  settings,
}: {
  pairing: string;
  settings: SettingsView;
}) {
  const {
    view,
    error,
    busy,
    update,
    comparison,
    selectProfile,
    save,
    cancel,
    stop,
  } = useCalibrationSession(pairing);
  const [confirmed, setConfirmed] = useState(false);
  const [trouble, setTrouble] = useState(false);
  const [editedName, setEditedName] = useState<string | null>(null);
  const name = editedName ?? view?.draft.name ?? '';
  function select(profile: string | null) {
    setEditedName(null);
    void selectProfile(profile);
  }
  function move(step: Step) {
    comparison(false);
    update({
      step,
      paused: step === 'colors',
      shade: step === 'grays' ? 3 : 0,
    });
  }
  const correction = view?.draft.correction ?? neutral;
  const headingRef = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    headingRef.current?.focus({ preventScroll: true });
  }, [view?.step, view?.ended]);
  const hueIndex = wheel.indexOf(colorNames[view?.shade ?? 0] ?? 'Red');
  const hue = correction.hue ?? [0, 0, 0, 0, 0, 0];
  function changeHue(value: number) {
    update({
      correction: {
        ...correction,
        hue: hue.map((v, i) => (i === hueIndex ? value : v)),
      },
    });
  }
  function slider(
    field: 'warmth' | 'tint' | 'midtones' | 'intensity' | 'hue',
    label: string,
    low: string,
    high: string,
  ) {
    const value =
      field === 'hue' ? (hue[hueIndex] ?? 0) : (correction[field] ?? 0);
    function change(v: number) {
      if (field === 'hue') changeHue(v);
      else update({ correction: { ...correction, [field]: v } });
    }
    return (
      <label className={`calibration-slider slider-${field}`}>
        <span className="slider-heading">
          <span>{label}</span>
          <output>
            {value > 0 ? '+' : ''}
            {value}
          </output>
        </span>
        <input
          type="range"
          min="-100"
          max="100"
          step="1"
          value={value}
          aria-label={label}
          onChange={(event) => change(Number(event.target.value))}
        />
        <span className="slider-endpoints" aria-hidden="true">
          <span>{low}</span>
          <span>{high}</span>
        </span>
      </label>
    );
  }
  function whiteBalance() {
    return (
      <>
        {slider('warmth', 'Temperature', 'Cooler', 'Warmer')}
        {slider('tint', 'Tint', 'Greener', 'More magenta')}
        <button
          className="calibration-text-button"
          onClick={() => {
            if (view)
              update({
                correction: { ...correction, warmth: 0, tint: 0 },
              });
          }}
        >
          Reset white balance
        </button>
      </>
    );
  }
  const stepIndex =
    view?.step === 'grays'
      ? steps.indexOf('colors')
      : steps.indexOf(view?.step ?? 'choose');
  if (view?.ended)
    return (
      <section className="calibration-wizard calibration-complete">
        <span className="completion-mark" aria-hidden="true">
          {view.saved ? '✓' : '—'}
        </span>
        <p className="eyebrow">
          {view.saved ? 'READY FOR YOUR NEXT SCREEN SYNC' : 'SESSION ENDED'}
        </p>
        <h1 ref={headingRef} tabIndex={-1}>
          {view.saved ? 'Your light. Your color.' : 'Calibration ended'}
        </h1>
        <p role="status">{view.ended}</p>
        {view.saved ? (
          <>
            <p className="saved-profile">{view.draft.name}</p>
            <p>
              Press Back on the TV when you’re finished checking your lights.
            </p>
          </>
        ) : null}
        <Link className="calibration-return" to="/overview">
          Return to Overview
        </Link>
        <button
          className="calibration-text-button danger"
          onClick={() => void stop()}
        >
          Stop lighting
        </button>
      </section>
    );
  return (
    <section className="calibration-wizard" aria-label="Match your lights">
      <div className="calibration-topline">
        <p className="eyebrow">MATCH YOUR LIGHTS</p>
        <span>
          {view ? `${stepIndex + 1} / ${steps.length}` : 'CONNECTING'}
        </span>
      </div>
      <ol className="calibration-progress" aria-label="Calibration progress">
        {[
          'Profile',
          'Response',
          'White balance',
          'Intensity',
          'Hues',
          'Mixed colors',
          'Save',
        ].map((label, i) => (
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
        {view ? titles[view.step] : 'Connecting to your TV…'}
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
                  disabled={busy}
                  onClick={() => select(null)}
                >
                  New profile
                </button>
                {(settings.settings.calibration?.profiles.length ?? 0) > 0 ? (
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
                      {(settings.settings.calibration?.profiles ?? []).map(
                        (profile) => (
                          <option key={profile.id} value={profile.id}>
                            {profile.name}
                          </option>
                        ),
                      )}
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
                <button
                  className="calibration-text-button"
                  aria-expanded={trouble}
                  onClick={() => setTrouble(!trouble)}
                >
                  The lights are not responding
                </button>
                {trouble ? (
                  <div className="calibration-note">
                    <p>
                      Check that WLED is powered and connected, Prysm uses the
                      webOS input, and the TV sender says Capturing. If the
                      video is missing, exit on the TV and try again.
                    </p>
                  </div>
                ) : null}
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
                <div className="calibration-adjustments">{whiteBalance()}</div>
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
            {view.step === 'grays' ? (
              <>
                <p className="calibration-lead">
                  Optional: change how strongly darker scenes light the wall.
                </p>
                <p className="calibration-tip">
                  This is a preference, not a brightness match. Leave it at zero
                  if neither direction looks clearly better.
                </p>
                <div
                  className="calibration-swatches gray-swatches"
                  role="group"
                  aria-label="TV shade"
                >
                  {[
                    'Black',
                    'Very dark',
                    'Dark',
                    'Mid gray',
                    'Light gray',
                    'White',
                  ].map((label, i) => (
                    <button
                      key={label}
                      aria-pressed={view.shade === i}
                      onClick={() => update({ shade: i })}
                    >
                      <i
                        style={{
                          background: [
                            '#000',
                            '#202020',
                            '#404040',
                            '#808080',
                            '#c0c0c0',
                            '#fff',
                          ][i],
                        }}
                      />
                      {label}
                    </button>
                  ))}
                </div>
                {slider('midtones', 'Dark-tone response', 'Darker', 'Lighter')}
                <button
                  className="calibration-text-button"
                  onClick={() =>
                    update({
                      correction: { ...correction, midtones: 0 },
                    })
                  }
                >
                  Reset dark tones
                </button>
                <p className="calibration-hint">
                  This affects brightness across all colors. It cannot fix a
                  tint that changes with brightness.
                </p>
              </>
            ) : null}
            {view.step === 'intensity' || view.step === 'mixed' ? (
              <>
                <p className="calibration-lead">
                  {view.step === 'intensity'
                    ? 'Try all three patches. If the wall looks washed out, increase color intensity. If it looks too vivid, lower it.'
                    : 'Compare the left and right sides of the wall with the same sides of the TV. Look for the two colors to stay distinct with your everyday settings.'}
                </p>
                <div
                  className="calibration-swatches"
                  role="group"
                  aria-label="TV pattern"
                >
                  {(view.step === 'intensity' ? muted : pairs).map(
                    ({ label, swatch }, i) => (
                      <button
                        key={label}
                        aria-pressed={view.shade === i}
                        onClick={() => update({ shade: i })}
                      >
                        <i style={{ background: swatch }} />
                        {label}
                      </button>
                    ),
                  )}
                </div>
                {view.step === 'intensity' ? (
                  <>
                    {slider(
                      'intensity',
                      'Color intensity',
                      'More muted',
                      'More vivid',
                    )}
                    <button
                      className="calibration-text-button"
                      onClick={() =>
                        update({ correction: { ...correction, intensity: 0 } })
                      }
                    >
                      Reset intensity
                    </button>
                  </>
                ) : (
                  <>
                    <p className="calibration-pattern" role="status">
                      {pairs[view.shade]?.left} on the left ·{' '}
                      {pairs[view.shade]?.right} on the right
                    </p>
                    <p className="calibration-tip">
                      Hold for uncorrected to compare. If one color looks off,
                      go back to Hues. If all colors feel too strong or weak,
                      revisit intensity.
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
                  Choose a color that looks off on the wall. Adjust its hue
                  toward the named neighbor until it looks closer to the TV.
                  Leave the others as they are.
                </p>
                <div
                  className="calibration-swatches"
                  role="group"
                  aria-label="TV color"
                >
                  {colorNames.map((color, i) => (
                    <button
                      key={color}
                      aria-pressed={view.paused && view.shade === i}
                      onClick={() => update({ shade: i, paused: true })}
                    >
                      <i
                        style={{
                          background: [
                            '#ed5454',
                            '#51b979',
                            '#5a7fea',
                            '#55c8ca',
                            '#cc69bb',
                            '#edcb64',
                          ][i],
                        }}
                      />
                      {color}
                    </button>
                  ))}
                </div>
                <p className="calibration-pattern" role="status">
                  {view.paused
                    ? `${colorNames[view.shade]} on your TV · stays until you change it`
                    : 'Cycling through all six colors'}
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
                    {slider(
                      'hue',
                      wheel[hueIndex] + ' hue',
                      'Toward ' + wheel[(hueIndex + 5) % 6],
                      'Toward ' + wheel[(hueIndex + 1) % 6],
                    )}
                    <button
                      className="calibration-text-button"
                      onClick={() => changeHue(0)}
                    >
                      Reset {wheel[hueIndex]?.toLowerCase()} hue
                    </button>
                  </div>
                ) : (
                  <p className="calibration-hint">
                    Choose a static color to adjust its hue.
                  </p>
                )}
                <details className="calibration-details">
                  <summary>Refine the overall color balance</summary>
                  <p>
                    These controls affect every color. Recheck the other colors
                    after a change. Some differences come from the LEDs and wall
                    and cannot be removed by white balance.
                  </p>
                  {whiteBalance()}
                </details>
                <details className="calibration-details">
                  <summary>Dark scenes feel too strong or too faint?</summary>
                  <p>
                    Usually, leave dark tones unchanged. Adjust only if you see
                    a consistent problem.
                  </p>
                  <button onClick={() => move('grays')}>
                    Adjust dark tones
                  </button>
                </details>
              </>
            ) : null}
            {view.step === 'review' ? (
              <>
                <p className="calibration-lead">
                  One last look with your usual lighting settings. Save when the
                  wall glow feels right.
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
            {view.step !== 'choose' ? (
              <button
                disabled={busy}
                onClick={() =>
                  move(
                    view.step === 'grays'
                      ? 'colors'
                      : (steps[Math.max(0, steps.indexOf(view.step) - 1)] ??
                          'choose'),
                  )
                }
              >
                Back
              </button>
            ) : null}
            {view.step === 'review' ? (
              <button
                className="primary"
                disabled={busy || !name.trim()}
                onClick={() => void save(name.trim())}
              >
                {busy ? 'Saving…' : 'Save and use'}
              </button>
            ) : view.step !== 'choose' ? (
              <button
                className="primary"
                disabled={busy || (view.step === 'response' && !confirmed)}
                onClick={() =>
                  move(
                    view.step === 'grays'
                      ? 'colors'
                      : (steps[steps.indexOf(view.step) + 1] ?? 'review'),
                  )
                }
              >
                {view.step === 'grays' ? 'Done' : 'Next'}
              </button>
            ) : null}
          </div>
        </>
      ) : (
        <p className="calibration-lead">
          {error
            ? 'Start Match your lights in the Prysm TV app to get a fresh link.'
            : 'Keep the Prysm app open on your TV.'}
        </p>
      )}
      <footer className="calibration-footer">
        {view ? (
          <button
            className="calibration-text-button"
            disabled={busy}
            onClick={() => void cancel()}
          >
            Cancel
          </button>
        ) : null}
        <button
          className="calibration-text-button danger"
          onClick={() => void stop()}
        >
          Stop lighting
        </button>
      </footer>
    </section>
  );
}
