/**
 * What the light card shows, decided from the lighting situation and what the
 * card remembers: pure, so the card's decisions are tested without a page.
 */
import {
  describeOutputIssue,
  describeSituation,
  plainFailure,
  situationFailure,
  type Failure,
  type LightingFailure,
  type LightingMode,
  type LightingSituation,
  type LightingTarget,
  type TargetReading,
} from '@prysm/application-client';
import type { DeviceReport } from '@prysm/contracts/application';
import { ledCountsDiffer, type OutputLighting } from './adapters';

/**
 * Below this share the lights' own brightness is named on the card. The
 * lights show Prysm's brightness times theirs; above about three quarters
 * the difference is hard to see (perceived lightness stays near 90 %), while
 * WLED's own default (128 of 255, 50 %) halves every colour unexplained.
 */
export const deviceBrightnessNoteBelow = 75;

/**
 * The state words, drawn in one place and cross-faded. `Unknown`: Prysm is
 * newer than this page and says something it cannot read.
 */
export const words = ['Off', 'On', 'Stopped', 'Unknown'] as const;
export type Word = (typeof words)[number];

/**
 * What the card shows: lit by screen sync or the effects, starting, stopped
 * by a problem, off, without the setup it needs, or a state newer than this
 * page.
 */
export type Mood =
  'off' | 'problem' | 'sync' | 'effects' | 'starting' | 'unset' | 'unknown';

export function moodOf(lighting: LightingSituation, configured: boolean): Mood {
  if (!configured) return 'unset';
  switch (lighting.kind) {
    case 'unknown':
      return 'unknown';
    case 'faulted':
    case 'failed':
      return 'problem';
    case 'sync':
      return 'sync';
    case 'effects':
      return 'effects';
    case 'starting':
      return 'starting';
    case 'off':
      return 'off';
  }
}

/** A pause worth naming once it lasts: no picture yet, or a slow start. */
export type Pause = 'waiting' | 'starting';

/** The pause the card is in now, named only once it lasts. */
export function pauseOf(
  lighting: LightingSituation,
  lit: boolean,
): Pause | null {
  if (lighting.kind === 'sync' && lighting.input && !lighting.input.receiving)
    return 'waiting';
  if (lighting.kind === 'starting' && !lit) return 'starting';
  return null;
}

/** The card's one informative line: a problem, a warning or a note. */
export interface Info {
  tone: 'problem' | 'warning' | 'note';
  text: string;
  /** Offer Try again: the requested mode failed. */
  retry?: boolean;
  /** The technical message, under Details. */
  details?: string;
}

/** Why screen sync cannot start yet, or `null`. */
function missingSetup(hasCapture: boolean, hasOutput: boolean) {
  if (!hasCapture && !hasOutput)
    return 'Set up a video source and lights first.';
  if (!hasCapture) return 'Set up a video source first.';
  if (!hasOutput) return 'Set up your lights first.';
  return null;
}

/** Why the output's own effects cannot start, or `null`. */
function missingEffects(hasOutput: boolean, effects: string | undefined) {
  if (!hasOutput) return 'Set up your lights first.';
  if (!effects) return 'These lights have no effects of their own.';
  return null;
}

export interface LightCardFacts {
  target: TargetReading;
  lighting: LightingSituation;
  /**
   * What the lights say about themselves while screen sync runs: why they
   * do not show it, their own brightness, how many LEDs they have.
   */
  device: DeviceReport;
  /** How many LEDs Prysm sends: the configured edges' total. */
  sentLedCount?: number;
  hasCapture: boolean;
  hasOutput: boolean;
  /** What the configured output offers, `null` without an output. */
  output: OutputLighting | null;
}

/** What the card remembers between states. */
export interface LightCardMemory {
  /** The mode the power button turns on, kept in this browser. */
  remembered: LightingMode;
  /**
   * A refused click, while the lights are as they were then: its failure
   * and the target it asked for.
   */
  error: { failure: Failure; asked: LightingTarget } | null;
  /** The light is up: running, or starting after it ran. */
  lit: boolean;
  /** The pause that has lasted long enough to name. */
  note: Pause | null;
}

export interface LightCardView {
  /** The output's own effects by name. */
  effects: string;
  on: boolean;
  /** Why each mode cannot be chosen now, or `null`. */
  reasons: Record<LightingMode, string | null>;
  /** The chosen mode: the one on, else the one the power button turns on. */
  mode: LightingMode;
  /** Why the power button cannot work now, or `null`. */
  powerReason: string | null;
  word: Word;
  palette: 'sync' | 'effects' | 'error';
  /** The state in plain words, for screen readers. */
  status: string;
  info: Info | null;
  /**
   * The calm line, said for sighted people (the status says it for screen
   * readers): what Power will do, a pause once it lasts, or what runs. Empty
   * while the card has something else to say, or Power cannot turn on.
   */
  calm: string;
  /**
   * With the lights' own effects chosen, Prysm's picture controls do not
   * reach the lights (the engine runs no session): one line says so, with
   * the way to the device's own controls.
   */
  ownEffects: boolean;
}

export function lightCardView(
  facts: LightCardFacts,
  { remembered, error, lit, note }: LightCardMemory,
): LightCardView {
  const { target, lighting, device, output } = facts;
  const effects = output?.effects ?? 'Device effects';
  const faulted = lighting.kind === 'faulted';
  // A target newer than this page turns no known mode on.
  const on = target === 'screen_sync' || target === 'effects';
  const configured = facts.hasCapture && facts.hasOutput;

  const reasonOf = (unavailable: string | null) =>
    faulted ? 'Restart Prysm to change the lights.' : unavailable;
  const reasons: Record<LightingMode, string | null> = {
    screen_sync: reasonOf(missingSetup(facts.hasCapture, facts.hasOutput)),
    effects: reasonOf(missingEffects(facts.hasOutput, output?.effects)),
  };
  let mode: LightingMode = on ? target : remembered;
  const other = mode === 'screen_sync' ? 'effects' : 'screen_sync';
  if (!on && reasons[mode] && !reasons[other]) mode = other;
  const powerReason = on ? reasonOf(null) : reasons[mode];

  const mood = moodOf(lighting, configured);
  // The word follows the light, not the request: "On" arrives with the light.
  let word: Word = lit ? 'On' : 'Off';
  if (mood === 'problem') word = 'Stopped';
  if (mood === 'unknown') word = 'Unknown';
  let palette: LightCardView['palette'] =
    mode === 'effects' ? 'effects' : 'sync';
  if (mood === 'problem') palette = 'error';
  const status = describeSituation(lighting, effects).headline;

  /** What went wrong: the latest command's own failure, else the state's. */
  // Faulted, the status says what to do instead.
  const failure = situationFailure(lighting);
  // A refused click is said by what it asked for; the state's by its target.
  let problem: {
    failure: Failure | LightingFailure;
    target: TargetReading;
  } | null = null;
  if (!faulted && error)
    problem = { failure: error.failure, target: error.asked };
  // With the effects on, a failure is settings that do not validate: a note.
  else if (!faulted && failure && lighting.kind !== 'effects')
    problem = { failure, target };
  // The card's one line of words, only when it says what the card does not.
  const showing =
    lit && lighting.kind === 'sync' && !!lighting.input?.receiving;
  const countsDiffer = ledCountsDiffer(device.led_count, facts.sentLedCount);
  const deviceBrightness = device.brightness_percent;
  let info: Info | null = null;
  if (problem)
    info = {
      tone: 'problem',
      text: plainFailure(problem.failure, problem.target, effects),
      details: problem.failure.message,
      retry: lighting.kind === 'failed',
    };
  else if (lighting.kind === 'effects' && lighting.failure)
    info = {
      tone: 'note',
      text: 'Prysm can’t use these settings.',
      details: lighting.failure.message,
    };
  else if (faulted)
    info = {
      tone: 'problem',
      text: 'Restart Prysm to recover.',
      details: lighting.message,
      retry: false,
    };
  else if (lighting.kind === 'unknown')
    info = {
      tone: 'note',
      text: 'Update it to see what the lights do.',
      details: lighting.message ?? undefined,
    };
  else if (device.issue && lighting.kind === 'sync')
    info = { tone: 'warning', text: describeOutputIssue(device.issue) };
  // While screen sync shows: LED counts that differ leave some LEDs dark or
  // some colours unshown, so they come first; then the lights' own
  // brightness, as a low one is named in place of "Following the screen.",
  // so Prysm at 100 % that looks dim has its reason. Prysm never changes the
  // lights' settings.
  else if (showing && countsDiffer)
    info = { tone: 'warning', text: countsDiffer };
  else if (
    showing &&
    deviceBrightness !== undefined &&
    deviceBrightness < deviceBrightnessNoteBelow
  )
    info = {
      tone: 'note',
      text: `${output?.effects?.replace(/ effects$/, '') ?? 'The lights'} limits brightness to ${deviceBrightness}%.`,
    };

  let calm = '';
  if (info || (!on && powerReason)) calm = '';
  else if (!on)
    calm =
      mode === 'screen_sync'
        ? 'Power turns on Screen sync.'
        : `Power turns on ${effects}.`;
  else if (note === 'waiting') calm = 'Waiting for the picture.';
  else if (note === 'starting') calm = 'Starting…';
  else if (showing) calm = 'Following the screen.';
  else if (lighting.kind === 'effects') calm = `${effects} are showing.`;

  return {
    effects,
    on,
    reasons,
    mode,
    powerReason,
    word,
    palette,
    status,
    info,
    calm,
    ownEffects: mode === 'effects' && !info && !(on && note),
  };
}
