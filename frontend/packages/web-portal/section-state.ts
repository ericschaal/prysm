import {
  describeOutputIssue,
  plainFailure,
  type Failure,
  type LightingTarget,
  type OutputIssue,
} from '@prysm/application-client';
import type { DeviceBrightness, Input } from '@prysm/contracts/application';
import { deviceBrightnessNoteBelow } from './lighting';
import type { PathState } from './setup-card';

/** What the state says about screen sync now, for a section's status. */
export interface LiveState {
  online: boolean;
  target: LightingTarget;
  path: PathState;
  failure: Failure | null;
  input?: Input;
  outputIssue?: OutputIssue;
  outputBrightness?: DeviceBrightness;
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
  effects = 'Device effects',
): Status {
  const capture = section === 'capture';
  const { path, failure, target, outputIssue } = live;
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
  if (path.kind === 'failed') {
    const plain = failure
      ? plainFailure(failure, target, effects)
      : 'Screen sync stopped.';
    if (path.at !== section && path.at !== null)
      return { tone: 'idle', title: 'Screen sync stopped', detail: plain };
    return capture
      ? {
          tone: 'problem',
          title: 'No picture',
          detail: '',
          details: failure?.message ?? plain,
        }
      : {
          tone: 'problem',
          title: 'Lights not responding',
          detail: identity,
          details: failure?.message,
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
  if (target === 'device_native') {
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
    const brightness = live.outputBrightness?.percent;
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
