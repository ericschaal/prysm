/** The full package's status card words, by its situation. */
import {
  outputIssueHeadline,
  plainFailure,
  unreachable,
  whenLightsAnswer,
  type LightingTarget,
  type OutputIssue,
} from '@prysm/application-client';
import type { Asked, Fix } from '@prysm/webos-platform';
import {
  noPictureCopy,
  packageWords,
  sharedCopy,
  sharedFixCopy,
  UNANSWERED,
  type Copy,
} from './copy';
import type { FullSituation, Headline } from './full';

/** The lights card says what to do. */
const SEE_LIGHTS = 'See Your lights for what to do.';

/** Full: what the lights do or what stopped, as a headline. */
const headlines: Record<Headline, string> = {
  prysmStopped: 'Prysm stopped',
  syncStopped: packageWords.full.stopped,
  syncNotStarted: 'Screen sync didn’t start',
  effectsStopped: 'WLED effects stopped',
  off: 'Lights off',
  effectsOn: 'WLED effects on',
  syncOn: 'Screen sync on',
};

/** Full: what the service asks, under `headline`. */
function askedCopy(asked: Asked, headline: Headline): Copy {
  // Prysm stopped trying: the lights card would say it keeps trying.
  if (asked.step === 'unanswered')
    return {
      title: UNANSWERED,
      description: 'Prysm stopped trying. Check them, then try again.',
      attention: true,
    };
  const title = headlines[headline];
  const fixes: Record<Fix, Copy> = {
    try_again: {
      title,
      description: 'Try again, or choose another mode.',
      attention: true,
    },
    // Trying again cannot help settings Prysm can't use.
    change_settings: {
      title,
      description: 'Change the settings in the web portal.',
      attention: true,
    },
    ...sharedFixCopy,
  };
  return fixes[asked.fix];
}

/**
 * The detail under an issue's headline, with `target` chosen. Only lights
 * that don't answer come back by themselves, and the target is applied
 * then; for every other issue (a newer one too) the person acts, as the
 * lights card says.
 */
function issueDetail(issue: OutputIssue, target: LightingTarget): string {
  switch (issue) {
    case 'unreachable':
      return whenLightsAnswer(target, 'WLED effects');
    case 'off':
    case 'overridden':
    case 'not_live':
    case 'other_sender':
    default:
      return SEE_LIGHTS;
  }
}

/** Full: the status card's words for its situation. */
export function fullCopy(situation: FullSituation): Copy {
  switch (situation.kind) {
    case 'stopping':
    case 'setUpLights':
    case 'updating':
    case 'unreadable':
    case 'asleep':
    case 'noVideo':
      return sharedCopy(situation, 'full');
    case 'waitingForVideo':
      return noPictureCopy(situation, 'full');
    case 'problem':
      return askedCopy(situation.asked, situation.headline);
    case 'faulted':
      // The service stops it next: a restart, or what it asks.
      return {
        title: 'Prysm stopped responding',
        description: 'This takes a moment.',
        attention: true,
      };
    case 'lightingFailed':
      // The lights card says what failed, and the service what to do next.
      return {
        title: headlines[situation.headline],
        description: SEE_LIGHTS,
        attention: true,
      };
    case 'newer':
      return {
        title: 'Prysm is newer than this app',
        description: 'Update the app to see what the lights do.',
        attention: false,
      };
    case 'off':
      // Still off, but its failure is said, as the lights card says it:
      // settings that do not validate, or a switch off that failed.
      return situation.failure
        ? {
            title: headlines.off,
            description: plainFailure(situation.failure, 'off', 'WLED effects'),
            attention: true,
          }
        : {
            title: headlines.off,
            description: 'Choose Screen sync: the lights follow the picture.',
            attention: false,
          };
    case 'effects':
      return {
        title: headlines.effectsOn,
        description: 'Your lights run their own effects.',
        attention: false,
      };
    case 'lightsIssue':
      // The card has no tone but attention: the portal's warning and
      // problem are both said with it.
      return {
        title: outputIssueHeadline(situation.issue),
        description: issueDetail(situation.issue, situation.target),
        attention: true,
      };
    case 'syncOn':
      return {
        title: headlines.syncOn,
        description: 'Your lights follow the picture.',
        attention: false,
      };
    case 'starting':
      return {
        title: 'Starting screen sync…',
        description: 'This takes a few seconds.',
        attention: false,
      };
    default:
      return unreachable(situation);
  }
}
