/** The slim package's status card words, by its situation. */
import { unreachable } from '@prysm/application-client';
import { noPictureCopy, packageWords, sharedCopy, type Copy } from './copy';
import type { SlimSituation } from './slim';

/** Slim: the status card's words for its situation. */
export function slimCopy(situation: SlimSituation): Copy {
  switch (situation.kind) {
    case 'stopping':
    case 'retrying':
    case 'setUpLights':
    case 'updating':
    case 'unreadable':
    case 'asleep':
    case 'noVideo':
      return sharedCopy(situation, 'slim');
    case 'waitingForVideo':
      return noPictureCopy(situation, 'slim');
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
    case 'replaced':
      return {
        title: 'Another TV took over',
        description: 'Stop and start capture to send from this TV.',
        attention: true,
      };
    case 'receiverFailed':
      return {
        title: 'Prysm stopped receiving',
        description: 'Its web portal on your computer says why.',
        attention: true,
      };
    case 'captureFailed':
      return {
        title: packageWords.slim.stopped,
        description: 'Stop and start capture again to retry.',
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
