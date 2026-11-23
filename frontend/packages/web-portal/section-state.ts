import {
  describeOutputIssue,
  plainFailure,
  type LightingFailure,
  type LightingSituation,
  type TargetReading,
} from '@prysm/application-client';
import type { DeviceReport, Input } from '@prysm/contracts/application';
import { ledCountsDiffer } from './adapters';
import { deviceBrightnessNoteBelow } from './light-card-view';
import type { PathState } from './setup-card';

/** What the state says about screen sync now, for a section's status. */
export interface LiveState {
  online: boolean;
  target: TargetReading;
  path: PathState;
  /** The failure the lighting situation says (see `situationFailure`). */
  failure: LightingFailure | null;
  /** What the lights are doing, decided once (`lightingSituation`). */
  situation: LightingSituation;
  input?: Input;
  /** What the lights say about themselves. */
  device: DeviceReport;
}

/** What the saved lights are, beyond their identity. */
export interface SavedLights {
  /** Their own effects' name, such as `WLED effects`. */
  effects?: string;
  /** How many LEDs Prysm sends them: the configured edges' total. */
  leds?: number;
}

/** Whether a section works now, in a few words. */
export interface Status {
  /** `problem`: the saved section does not work; the way back is offered. */
  tone: 'ok' | 'busy' | 'idle' | 'warning' | 'problem';
  title: string;
  detail: string;
  /** Why it fails, under the line: Prysm's own words where it has any. */
  details?: string;
  /** Frames are flowing from this source: show them. */
  picture?: boolean;
}

/**
 * The video source's or the lights' status, from the live state alone.
 * `identity` names the saved section (`WLED at wled.local · 120 LEDs`), or is
 * `null` while it is not set up; `stalled`: waiting for the picture has
 * lasted too long to be a start.
 */
export function sectionStatus(
  section: 'capture' | 'output',
  live: LiveState,
  identity: string | null,
  stalled: boolean,
  lights: SavedLights = {},
): Status {
  const capture = section === 'capture';
  const effects = lights.effects ?? 'Device effects';
  const { path, failure, target, situation } = live;
  const outputIssue = live.device.issue;
  if (identity === null)
    return {
      tone: 'idle',
      title: 'Not set up yet',
      detail: capture
        ? 'Save a source below to start using it.'
        : 'Save the lights below to start using them.',
    };
  if (!live.online)
    return { tone: 'idle', title: 'Waiting for the connection…', detail: '' };
  // A target or session state newer than this page: never "Starting…".
  if (situation.kind === 'unknown')
    return {
      tone: 'idle',
      title: 'Prysm is newer than this page',
      detail: 'Reload it to see what the lights do.',
    };
  // Why: the fault's own words, else the failure's message.
  const reason =
    situation.kind === 'faulted' ? situation.message : failure?.message;
  // With the lights off nothing runs: a failure is said only on the section
  // it names (its settings do not validate, or a switch off failed); the
  // other one is not in use.
  const off = situation.kind === 'off';
  if (path.kind === 'failed' && (path.at === section || !off)) {
    const plain = failure
      ? plainFailure(failure, target, effects)
      : 'Screen sync stopped.';
    if (path.at !== section && path.at !== null)
      return { tone: 'idle', title: 'Screen sync stopped', detail: plain };
    if (off && failure?.kind === 'invalid')
      return {
        tone: 'problem',
        title: 'Can’t use these settings',
        detail: capture ? '' : identity,
        details: reason,
      };
    if (off && capture)
      return {
        tone: 'problem',
        title: 'Video source not working',
        detail: '',
        details: reason,
      };
    return capture
      ? {
          tone: 'problem',
          title: 'No picture',
          detail: '',
          details: reason ?? plain,
        }
      : {
          tone: 'problem',
          title: 'Lights not responding',
          detail: identity,
          details: reason,
        };
  }
  if (target === 'off')
    return {
      tone: 'idle',
      title: 'Not in use',
      detail: capture
        ? 'Screen sync is off, so this source isn’t in use.'
        : 'Screen sync is off, so these lights aren’t in use.',
    };
  if (target === 'effects') {
    if (capture)
      return {
        tone: 'idle',
        title: 'Not in use',
        detail: `The lights show ${effects}, so this source isn’t in use.`,
      };
    return path.kind === 'effects'
      ? { tone: 'ok', title: `Showing ${effects}`, detail: identity }
      : { tone: 'busy', title: 'Starting…', detail: identity };
  }
  // Screen sync: the lights say first when they do not show it.
  if (!capture && outputIssue && path.kind !== 'off')
    return outputIssue === 'unreachable'
      ? {
          tone: 'problem',
          title: 'Can’t reach the lights',
          detail: `${identity}. ${describeOutputIssue(outputIssue)}`,
        }
      : {
          tone: 'warning',
          title: 'Not showing screen sync',
          detail: describeOutputIssue(outputIssue),
        };
  if (path.kind === 'flowing') {
    if (capture)
      return {
        tone: 'ok',
        title: 'Picture arriving',
        detail: `${Math.round(path.fps)} fps`,
        picture: true,
      };
    const differ = ledCountsDiffer(live.device.led_count, lights.leds);
    if (differ)
      return { tone: 'warning', title: 'LED counts differ', detail: differ };
    const brightness = live.device.brightness_percent;
    return {
      tone: 'ok',
      title: 'Showing screen sync',
      detail:
        brightness !== undefined && brightness < deviceBrightnessNoteBelow
          ? `${identity} · brightness limited to ${brightness}% on the device`
          : identity,
    };
  }
  if (path.kind === 'waiting' && stalled)
    return capture
      ? {
          tone: 'problem',
          title: 'No picture',
          detail: '',
          details: 'Nothing has arrived from this source.',
        }
      : {
          tone: 'idle',
          title: 'Waiting for the picture',
          detail: 'The video source sends nothing yet.',
        };
  return {
    tone: 'busy',
    title: 'Starting…',
    detail: capture ? 'Waiting for the picture.' : identity,
  };
}
