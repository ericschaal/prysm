/** The slim package's status card words, by its situation. */
import { unreachable } from '@prysm/application-client';
import type { Asked, Fix } from '@prysm/webos-platform';
import {
  noPictureCopy,
  packageWords,
  sharedCopy,
  sharedFixCopy,
  UNANSWERED,
  type Copy,
} from './copy';
import type { SlimSituation } from './slim';

/** Slim: what the service asks; Try again starts capture again. */
function askedCopy(asked: Asked): Copy {
  if (asked.step === 'unanswered')
    return {
      title: UNANSWERED,
      description: 'Prysm stopped trying. Check Prysm, then try again.',
      attention: true,
    };
  const fixes: Record<Fix, Copy> = {
    try_again: {
      title: packageWords.slim.stopped,
      description: 'Something went wrong. Try again to restart it.',
      attention: true,
    },
    // The picture settings are Prysm's, set in its portal.
    change_settings: {
      title: packageWords.slim.stopped,
      description: 'Change the settings in Prysm’s web portal.',
      attention: true,
    },
    ...sharedFixCopy,
  };
  return fixes[asked.fix];
}

/** Slim: the status card's words for its situation. */
export function slimCopy(situation: SlimSituation): Copy {
  switch (situation.kind) {
    case 'stopping':
    case 'setUpLights':
    case 'updating':
    case 'unreadable':
    case 'asleep':
    case 'noVideo':
      return sharedCopy(situation, 'slim');
    case 'waitingForVideo':
      return noPictureCopy(situation, 'slim');
    case 'problem':
      return askedCopy(situation.asked);
    case 'encoder':
      return {
        title: 'Encoding not supported',
        description: 'Choose another encoding in Prysm’s web portal.',
        attention: true,
      };
    case 'version':
      return {
        title: 'Update Prysm or TV app',
        description: 'Update the older of Prysm and this app.',
        attention: true,
      };
    case 'receiverFailed':
      return {
        title: 'Prysm stopped receiving',
        description: 'Its web portal on your computer says why.',
        attention: true,
      };
    case 'noAddress':
      return {
        title: 'Set up this TV',
        description: 'Add the address of Prysm on your computer.',
        attention: false,
      };
    case 'captureOff':
      return {
        title: 'Capture off',
        description: 'Start capture to send the picture to Prysm.',
        attention: false,
      };
    case 'paused':
      return {
        title: 'Paused by Prysm',
        description: 'It continues when Prysm turns the lights on.',
        attention: false,
      };
    case 'waitingForPrysm':
      return {
        title: 'Waiting for Prysm',
        description: 'Check that Prysm is running on your computer.',
        attention: false,
      };
    case 'sending':
      return {
        title: 'Sending the picture',
        description: 'Prysm on your computer lights up with it.',
        attention: false,
      };
    default:
      return unreachable(situation);
  }
}
