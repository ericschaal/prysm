/**
 * What the light card shows, decided from the lighting situation and what the
 * card remembers: pure, so the card's decisions are tested without a page.
 */
import {
  describeOutputIssue,
  describeSituation,
  pictureOf,
  plainFailure,
  REFUSED_VIDEO,
  situationFailure,
  unreachable,
  unusableSettings,
  type DeviceReport,
  type Failure,
  type LightingFailure,
  type LightingMode,
  type LightingSituation,
  type LightingTarget,
  type Picture,
  type TargetReading,
} from '@prysm/application-client';
import type { LightingControl } from '@prysm/contracts/application';
import { ledCountsDiffer, type OutputLighting } from './adapters';

/** Device brightness below this percentage is eligible for a note on the light card. */
export const deviceBrightnessNoteBelow = 75;

export const words = ['Off', 'On', 'Stopped', 'Unknown'] as const;
export type Word = (typeof words)[number];

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
    // Lights waited for are neither lit nor busy: the info line says it.
    case 'unanswered':
    case 'off':
      return 'off';
    default:
      return unreachable(lighting);
  }
}

/**
 * A pause worth naming once it lasts: no picture, and why (`quiet` when the
 * source gives none), or a slow start.
 */
export type Pause = Exclude<Picture, 'arriving'> | 'starting';

/** The calm line while no picture arrives; neither reason is a problem. */
const waitingLine: Record<Exclude<Picture, 'arriving'>, string> = {
  quiet: 'Waiting for the picture.',
  refused: REFUSED_VIDEO,
};

/** The pause the card is in now, named only once it lasts. */
export function pauseOf(
  lighting: LightingSituation,
  lit: boolean,
): Pause | null {
  if (lighting.kind === 'sync' && lighting.input) {
    const picture = pictureOf(lighting.input);
    if (picture !== 'arriving') return picture;
  }
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
  control?: LightingControl;
  lighting: LightingSituation;
  /**
   * What the lights say about themselves while screen sync runs: why they
   * do not show it, their own brightness, how many LEDs they have. Lights
   * that don't answer Off or their effects are the situation `unanswered`.
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
  /** No mode tile is selected under external control or for an unknown target. */
  chosen: LightingMode | null;
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
  { error, lit, note }: LightCardMemory,
): LightCardView {
  const { target, lighting, device, output, control } = facts;
  const remembered =
    control?.remembered ?? (output?.effects ? 'effects' : 'screen_sync');
  const effects = output?.effects ?? 'Device effects';
  const faulted = lighting.kind === 'faulted';
  const on = !!control?.external || target !== 'off';
  const known = target === 'screen_sync' || target === 'effects';
  const configured = facts.hasCapture && facts.hasOutput;

  const reasonOf = (unavailable: string | null) =>
    faulted ? 'Restart Prysm to change the lights.' : unavailable;
  const reasons: Record<LightingMode, string | null> = {
    screen_sync: reasonOf(missingSetup(facts.hasCapture, facts.hasOutput)),
    effects: reasonOf(missingEffects(facts.hasOutput, output?.effects)),
  };
  let mode: LightingMode = known ? target : remembered;
  const other = mode === 'screen_sync' ? 'effects' : 'screen_sync';
  if (!on && reasons[mode] && !reasons[other]) mode = other;
  const powerReason = on ? reasonOf(null) : reasons[mode];

  const mood = moodOf(lighting, configured);
  let word: Word = on ? 'On' : 'Off';
  if (mood === 'unknown') word = 'Unknown';
  const chosen = control?.external || target === 'unknown' ? null : mode;
  let palette: LightCardView['palette'] =
    chosen === 'effects' ? 'effects' : 'sync';
  if (mood === 'problem') palette = 'error';
  let status = control?.external
    ? 'Live stream detected.'
    : describeSituation(lighting, effects).headline;

  if (control?.waiting)
    status =
      control?.waiting === 'capture'
        ? 'Waiting for the video source.'
        : 'Waiting for the lights.';

  /** What went wrong: the latest command's own failure, else the state's. */
  const failure = situationFailure(lighting);
  // A refused click is said by what it asked for; the state's by its target.
  let problem: {
    failure: Failure | LightingFailure;
    target: TargetReading;
  } | null = null;
  if (!faulted && error)
    problem = { failure: error.failure, target: error.asked };
  else if (
    !faulted &&
    failure &&
    lighting.kind !== 'effects' &&
    lighting.kind !== 'unknown'
  )
    problem = { failure, target };
  // A start of screen sync that failed from the effects leaves them on.
  const stillOn =
    lighting.kind === 'failed' && lighting.effectsOn
      ? ` ${effects} are still on.`
      : '';
  const showing =
    lit &&
    lighting.kind === 'sync' &&
    !!lighting.input &&
    pictureOf(lighting.input) === 'arriving';
  const countsDiffer = ledCountsDiffer(device.led_count, facts.sentLedCount);
  const deviceBrightness = device.brightness_percent;
  let info: Info | null = null;
  if (control?.waiting) info = { tone: 'note', text: status };
  else if (problem)
    info = {
      tone: 'problem',
      text: `${plainFailure(problem.failure, problem.target, effects)}${stillOn}`,
      details: problem.failure.message,
      retry:
        on && lighting.kind === 'failed' && !unusableSettings(problem.failure),
    };
  else if (lighting.kind === 'effects' && lighting.failure)
    info =
      lighting.failure.kind === 'invalid'
        ? {
            tone: 'note',
            text: 'Prysm can’t use these settings.',
            details: lighting.failure.message,
          }
        : {
            tone: 'problem',
            text: plainFailure(lighting.failure, target, effects),
            details: lighting.failure.message,
          };
  else if (faulted)
    info = {
      tone: 'problem',
      text: 'Prysm stopped responding. Restart Prysm to recover.',
      details: lighting.message,
      retry: false,
    };
  else if (lighting.kind === 'unknown') {
    // The note stays; what the state carries goes with it.
    const said = lighting.failure
      ? ` ${plainFailure(lighting.failure, 'unknown', effects)}`
      : '';
    const details = [lighting.failure?.message, lighting.message]
      .filter((text) => !!text)
      .join(' ');
    info = {
      tone: 'note',
      text: `Prysm is newer than this page. Reload it to see what the lights do.${said}`,
      details: details || undefined,
    };
  } else if (device.issue && lighting.kind === 'sync')
    info = { tone: 'warning', text: describeOutputIssue(device.issue) };
  else if (lighting.kind === 'unanswered')
    info = {
      tone: 'warning',
      text: describeSituation(lighting, effects).sentence,
    };
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
  else if (control?.external) calm = status;
  else if (!on)
    calm =
      mode === 'screen_sync'
        ? 'Power turns on Screen sync.'
        : `Power turns on ${effects}.`;
  else if (note === 'starting') calm = 'Starting…';
  else if (note) calm = waitingLine[note];
  else if (showing) calm = 'Following the screen.';
  else if (lighting.kind === 'effects') calm = `${effects} are showing.`;

  return {
    effects,
    on,
    reasons,
    mode,
    chosen,
    powerReason,
    word,
    palette,
    status,
    info,
    calm,
    ownEffects: on && chosen === 'effects' && !info && !note,
  };
}
