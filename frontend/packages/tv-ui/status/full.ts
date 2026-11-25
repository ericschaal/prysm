/**
 * The full package's status card: what it is about, in order, and its
 * button. The lights are this TV's own portal's.
 */
import {
  pictureOf,
  unreachable,
  type LightingFailure,
  type LightingSituation,
  type OutputIssue,
} from '@prysm/application-client';
import {
  fullLights,
  fullPortal,
  isCapturing,
  problemOf,
  type Asked,
  type LightingTarget,
  type State,
} from '@prysm/webos-platform';
import {
  noLights,
  remedyOf,
  tvSituation,
  type Button,
  type FullState,
  type NoPicture,
  type PortalFacts,
  type SharedSituation,
} from './situation';

/**
 * Full: why the lights don't show screen sync, or that they don't answer
 * whatever was chosen, as the lights card says it: by the portal's state
 * once it is known (while screen sync runs or the lights are waited for),
 * as the worker last said before.
 */
function lightsIssue(
  state: State,
  portal: PortalFacts | null,
): OutputIssue | null {
  if (!portal?.lighting) return fullLights(state);
  const { kind } = portal.lighting;
  return kind === 'sync' || kind === 'unanswered'
    ? (portal.issue ?? null)
    : null;
}

/** The target a running, starting or waiting situation was asked for. */
function targetOf(
  lighting: Exclude<
    LightingSituation,
    { kind: 'faulted' | 'failed' | 'unknown' }
  >,
): LightingTarget {
  switch (lighting.kind) {
    case 'off':
      return 'off';
    case 'sync':
      return 'screen_sync';
    case 'effects':
      return 'effects';
    case 'unanswered':
    case 'starting':
      return lighting.target;
    default:
      return unreachable(lighting);
  }
}

/**
 * Full: what the lights do or what stopped, the headline of a failure: the
 * portal's word while it answers, else the target's (with no worker,
 * Prysm itself stopped).
 */
export type Headline =
  | 'prysmStopped'
  | 'syncStopped'
  | 'syncNotStarted'
  | 'effectsStopped'
  | 'off'
  | 'effectsOn'
  | 'syncOn';

function headlineOf(
  state: FullState,
  lighting: LightingSituation | undefined,
): Headline {
  const said = lighting && lightingHeadline(lighting);
  if (said) return said;
  switch (state.target) {
    case 'screen_sync':
      return 'syncStopped';
    case 'effects':
      return 'effectsStopped';
    case 'off':
      return state.worker.phase === 'asking' ? 'prysmStopped' : 'off';
    case 'unknown':
      return 'prysmStopped';
    default:
      return unreachable(state.target);
  }
}

/** What the lights do or what failed, by the portal; `null` when it says neither. */
function lightingHeadline(lighting: LightingSituation): Headline | null {
  switch (lighting.kind) {
    case 'failed':
      if (lighting.target === 'effects') return 'effectsStopped';
      return lighting.fromEffects ? 'syncNotStarted' : 'syncStopped';
    case 'off':
      return 'off';
    case 'effects':
      return 'effectsOn';
    case 'sync':
      return 'syncOn';
    case 'faulted':
    case 'unknown':
    case 'unanswered':
    case 'starting':
      return null;
    default:
      return unreachable(lighting);
  }
}

/** Full: what the status card is about ({@link fullSituation}). */
export type FullSituation =
  | SharedSituation
  | NoPicture
  /** What the service asks, under what the lights do or what stopped. */
  | { kind: 'problem'; asked: Asked; headline: Headline }
  | { kind: 'faulted' }
  /** The portal says it failed; the service does not say yet what is asked. */
  | { kind: 'lightingFailed'; headline: Headline }
  | { kind: 'newer' }
  | { kind: 'off'; failure: LightingFailure | null }
  | { kind: 'effects' }
  /** `target`: what was chosen; with Off or effects, only `unreachable`. */
  | { kind: 'lightsIssue'; issue: OutputIssue; target: LightingTarget }
  | { kind: 'syncOn' }
  | { kind: 'starting' };

/**
 * Full: what the status card is about, by the portal's situation once it is
 * known. The first that applies is said.
 */
export function fullSituation(
  state: FullState,
  portal: PortalFacts | null,
): FullSituation {
  const phase = state.worker.phase;
  const lighting = portal?.lighting;
  const problem = problemOf(state);
  // A stop that asks says what it asks; any other is a restart.
  if (phase === 'stopping' && !problem) return { kind: 'stopping' };
  if (noLights(portal)) return { kind: 'setUpLights' };
  if (phase === 'unknown') return { kind: 'updating' };
  // What the service asks, or a faulted or failed session, comes before
  // the TV's power and video. It is what needs the user; waking the TV or
  // a picture would not end it, and saying "Waiting for video" over it
  // would hide it. Lights that are fine (off, effects, screen sync) let
  // power and video speak first. The service's word is the one fix, also
  // with the lights off, and before lights the portal says it waits for:
  // Prysm stopped trying.
  if (problem)
    return {
      kind: 'problem',
      asked: problem.asked,
      headline: headlineOf(state, lighting),
    };
  if (lighting?.kind === 'faulted') return { kind: 'faulted' };
  if (lighting?.kind === 'failed')
    return { kind: 'lightingFailed', headline: headlineOf(state, lighting) };
  const tv = tvSituation(state);
  if (tv) return tv;
  if (lighting?.kind === 'unknown' || (!lighting && state.target === 'unknown'))
    return { kind: 'newer' };
  // The supervisor hears of a change made in the portal a moment later.
  const target = lighting ? targetOf(lighting) : state.target;
  // The lights don't show what was chosen; the lights card says why, as
  // for a refused video. With the lights off or their effects, only lights
  // that don't answer: the target stays and is applied once they answer.
  const issue = lightsIssue(state, portal);
  if (issue === 'unreachable' && (target === 'off' || target === 'effects'))
    return { kind: 'lightsIssue', issue, target };
  if (target === 'off')
    return {
      kind: 'off',
      failure: lighting?.kind === 'off' ? lighting.failure : null,
    };
  if (target === 'effects') return { kind: 'effects' };
  // Screen sync runs (also while it waits for lights that don't answer).
  if (issue) return { kind: 'lightsIssue', issue, target: 'screen_sync' };
  // The portal's measure; it alone says why no picture arrives.
  const picture =
    lighting?.kind === 'sync' && lighting.input
      ? pictureOf(lighting.input)
      : undefined;
  // Without the portal's measure yet, the TV's own word.
  const flowing = lighting
    ? lighting.kind === 'sync' &&
      (picture ? picture === 'arriving' : isCapturing(state))
    : isCapturing(state);
  if (flowing) return { kind: 'syncOn' };
  if (picture === 'refused')
    return { kind: 'waitingForVideo', picture: 'refused' };
  // Lighting is on but no frames arrive: the lights are released to WLED.
  if (fullPortal(state) && phase === 'running')
    return { kind: 'waitingForVideo', picture: 'quiet' };
  return { kind: 'starting' };
}

/**
 * Full: the modes start and stop the lights, so the button only answers
 * what the card says the service asks, as its fix says: try again (the
 * target again), set up the service, or nothing that could help. Nothing
 * while the worker stops, or for a target newer than this app (nothing to
 * send).
 */
export function fullButton(state: FullState, situation: FullSituation): Button {
  if (situation.kind !== 'problem' || state.worker.phase === 'stopping')
    return 'none';
  const remedy = remedyOf(situation.asked);
  return remedy === 'retry' && state.target === 'unknown' ? 'none' : remedy;
}
