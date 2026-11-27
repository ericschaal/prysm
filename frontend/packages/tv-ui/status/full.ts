import {
  outputIssueHeadline,
  pictureOf,
  plainFailure,
  unreachable,
  whenLightsAnswer,
  type LightingSituation,
} from '@prysm/application-client';
import {
  fullLights,
  fullPortal,
  isCapturing,
  problemOf,
  type LightingTarget,
  type State,
} from '@prysm/webos-platform';
import {
  noLights,
  remedyOf,
  type FullState,
  type PortalFacts,
} from './situation';
import {
  note,
  warning,
  refusedVideo,
  problemCopy,
  setupLights,
  tvCopy,
  updating,
  type Copy,
} from './copy';

/**
 * Full: why the lights don't show screen sync, or that they don't answer
 * whatever was chosen, as the lights card says it: by the portal's state
 * once it is known (while screen sync runs), as the worker last said
 * before.
 */
function lightsIssue(state: State, portal: PortalFacts | null): string | null {
  if (!portal?.lighting) return fullLights(state);
  return portal.lighting.kind === 'sync' ? (portal.issue ?? null) : null;
}

/**
 * Full: what the lights do or what stopped, the headline of a failure: the
 * portal's word while it answers, else the target's (with no worker,
 * Prysm itself stopped).
 */
function headlineOf(
  state: FullState,
  lighting: LightingSituation | undefined,
): string {
  switch (lighting?.kind) {
    case 'failed':
      if (lighting.target === 'effects') return 'WLED effects stopped';
      return lighting.fromEffects
        ? 'Screen sync didn’t start'
        : 'Screen sync stopped';
    case 'off':
      return 'Lights off';
    case 'effects':
      return 'WLED effects on';
    case 'sync':
      return 'Screen sync on';
  }
  switch (state.target) {
    case 'screen_sync':
      return 'Screen sync stopped';
    case 'effects':
      return 'WLED effects stopped';
    case 'off':
      return state.worker.phase === 'asking' ? 'Prysm stopped' : 'Lights off';
    case 'unknown':
      return 'Prysm stopped';
    default:
      return unreachable(state.target);
  }
}

const seeLights = 'See Your lights for what to do.';

function issueCopy(issue: string, target: LightingTarget): Copy {
  return warning(
    outputIssueHeadline(issue),
    issue === 'unreachable'
      ? whenLightsAnswer(target, 'WLED effects')
      : seeLights,
  );
}

function starting(target: LightingTarget): Copy {
  return note(
    target === 'effects' ? 'Starting WLED effects…' : 'Starting screen sync…',
    'This takes a few seconds.',
  );
}

export function fullStatus(state: FullState, portal: PortalFacts | null): Copy {
  const phase = state.worker.phase;
  const lighting = portal?.lighting;
  const problem = problemOf(state);
  if (phase === 'stopping' && !problem)
    return note('Restarting Prysm…', 'This takes a moment.');
  if (noLights(portal)) return setupLights;
  if (phase === 'unknown') return updating;
  if (problem) {
    const remedy = remedyOf(problem.asked);
    const disabled =
      phase === 'stopping' ||
      (remedy === 'retry' && state.target === 'unknown');
    return {
      ...problemCopy(problem.asked, headlineOf(state, lighting), 'full'),
      button: disabled ? 'none' : remedy,
    };
  }
  if (lighting?.kind === 'faulted')
    return warning('Prysm stopped responding', 'This takes a moment.');
  const waiting = portal?.control?.waiting;
  if (waiting)
    return note(
      waiting === 'capture' ? 'Waiting for video' : 'Waiting for the lights',
      'Press Power to cancel.',
    );
  if (portal?.control?.external)
    return note('Live stream detected', 'Choose a mode to control the lights.');
  if (lighting?.kind === 'failed')
    return warning(headlineOf(state, lighting), seeLights);
  const tv = tvCopy(state);
  if (tv) return tv;
  if (lighting?.kind === 'unknown' || (!lighting && state.target === 'unknown'))
    return note(
      'Prysm is newer than this app',
      'Update the app to see what the lights do.',
    );
  if (lighting?.kind === 'starting') return starting(lighting.target);
  if (lighting?.kind === 'unanswered')
    return issueCopy('unreachable', lighting.target);
  // Until the first portal state, use the service's last reported target and issue.
  const target = lighting ? null : state.target;
  const issue = lightsIssue(state, portal);
  if (issue === 'unreachable' && (target === 'off' || target === 'effects'))
    return issueCopy(issue, target);
  if (lighting?.kind === 'off' && lighting.failure)
    return warning(
      'Lights off',
      plainFailure(lighting.failure, 'off', 'WLED effects'),
    );
  if (lighting?.kind === 'off' || target === 'off')
    return note(
      'Lights off',
      'Choose Screen sync: the lights follow the picture.',
    );
  if (lighting?.kind === 'effects' || target === 'effects')
    return note('WLED effects on', 'Your lights run their own effects.');
  if (issue) return issueCopy(issue, 'screen_sync');
  const picture = lighting?.input ? pictureOf(lighting.input) : undefined;
  const flowing = picture ? picture === 'arriving' : isCapturing(state);
  if (flowing) return note('Screen sync on', 'Your lights follow the picture.');
  if (picture === 'refused') return refusedVideo('full');
  if (fullPortal(state) && phase === 'running')
    return note(
      'Waiting for video',
      'Screen sync starts when a picture arrives.',
    );
  return starting('screen_sync');
}
