/** The full package's status card words, by its situation. */
import {
  outputIssueHeadline,
  plainFailure,
  unreachable,
  unusableSettings,
  whenLightsAnswer,
  type LightingSituation,
  type LightingTarget,
  type OutputIssue,
} from '@prysm/application-client';
import { noPictureCopy, packageWords, sharedCopy, type Copy } from './copy';
import type { FullSituation } from './full';

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
      return 'See Your lights for what to do.';
  }
}

/** Full: the status card's words for its situation. */
export function fullCopy(situation: FullSituation): Copy {
  switch (situation.kind) {
    case 'stopping':
    case 'retrying':
    case 'setUpLights':
    case 'updating':
    case 'unreadable':
    case 'asleep':
    case 'noVideo':
      return sharedCopy(situation, 'full');
    case 'waitingForVideo':
      return noPictureCopy(situation, 'full');
    case 'sessionFailed':
      return {
        title: packageWords.full.stopped,
        description: 'Try again, or choose another mode.',
        attention: true,
      };
    case 'faulted':
      return {
        title: 'Prysm stopped responding',
        description: 'Restart Prysm to recover.',
        attention: true,
      };
    case 'lightingFailed':
      // The lights card says what failed; its modes try again.
      return {
        title: failedTitle(situation.lighting),
        // Trying again cannot help settings Prysm can't use.
        description: unusableSettings(situation.lighting.failure)
          ? 'Change the settings in the web portal.'
          : 'Try again, or choose another mode.',
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
            title: 'Lights off',
            description: plainFailure(situation.failure, 'off', 'WLED effects'),
            attention: true,
          }
        : {
            title: 'Lights off',
            description: 'Choose Screen sync: the lights follow the picture.',
            attention: false,
          };
    case 'effects':
      return {
        title: 'WLED effects on',
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
        title: 'Screen sync on',
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

/** Full: what failed, by the mode that was asked for. */
function failedTitle(
  lighting: Extract<LightingSituation, { kind: 'failed' }>,
): string {
  if (lighting.target === 'effects') return 'WLED effects stopped';
  return lighting.fromEffects
    ? 'Screen sync couldn’t start'
    : packageWords.full.stopped;
}
