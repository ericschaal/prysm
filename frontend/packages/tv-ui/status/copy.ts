/**
 * The status card's words that both packages use, and the primary
 * button's labels.
 */
import { unreachable, type Picture } from '@prysm/application-client';
import type { Flavor } from '@prysm/webos-platform';
import type {
  Button,
  NoPicture,
  PrimaryAction,
  SharedSituation,
} from './situation';

/**
 * The title while the TV refuses to share the video it shows; the lights
 * card says why (`REFUSED_VIDEO`), as the title and detail fit one line each.
 */
const REFUSED = 'Video can’t be captured';

/**
 * The status card while the capture gives no picture, by why and package
 * (slim's `quiet` also while its worker has not reported yet). Neither is a
 * failure: no button but slim's usual one, and a refusal ends by itself
 * when the video changes.
 */
const waitingStatus: Record<
  Exclude<Picture, 'arriving'>,
  Record<Flavor, { title: string; description: string }>
> = {
  quiet: {
    full: {
      title: 'Waiting for video',
      description: 'Screen sync starts when a picture arrives.',
    },
    slim: {
      title: 'Starting capture…',
      description: 'Waiting for the first picture.',
    },
  },
  refused: {
    full: {
      title: REFUSED,
      description: 'Screen sync continues when the video changes.',
    },
    slim: {
      title: REFUSED,
      description: 'Capture continues when the video changes.',
    },
  },
};

/** The status card's words, without its button. */
export interface Copy {
  title: string;
  description: string;
  attention: boolean;
}

/** Before a state: waiting for the service's first reply, or no service. */
export const linkCopy: Record<'connecting' | 'missing', Copy> = {
  connecting: {
    title: 'Connecting…',
    description: 'Checking Prysm on this TV.',
    attention: false,
  },
  missing: {
    title: 'Prysm needs setting up',
    description: 'Set up its service on this TV to start.',
    attention: true,
  },
};

/** The primary button's label and what it does. */
export const buttons: Record<Button, { label: string; action: PrimaryAction }> =
  {
    none: { label: '', action: 'none' },
    wait: { label: 'Please wait…', action: 'none' },
    setup: { label: 'Set up service', action: 'setup' },
    retry: { label: 'Try again', action: 'retry' },
    stop: { label: 'Stop capture', action: 'disable' },
    address: { label: 'Add Prysm address', action: 'address' },
    start: { label: 'Start capture', action: 'enable' },
  };

/** What the package stops, stopped, and continues when the TV wakes up. */
export const packageWords: Record<
  Flavor,
  { stopping: string; stopped: string; wakes: string }
> = {
  full: {
    stopping: 'Restarting Prysm…',
    stopped: 'Screen sync stopped',
    wakes: 'Screen sync continues when the TV wakes up.',
  },
  slim: {
    stopping: 'Stopping capture…',
    stopped: 'Capture stopped',
    wakes: 'Capture continues when the TV wakes up.',
  },
};

/** The words of a situation both packages have, in `flavor`'s words. */
export function sharedCopy(situation: SharedSituation, flavor: Flavor): Copy {
  const words = packageWords[flavor];
  switch (situation.kind) {
    case 'stopping':
      return {
        title: words.stopping,
        description: 'This takes a moment.',
        attention: false,
      };
    case 'retrying':
      return {
        title: words.stopped,
        description: 'Something went wrong. Prysm tries again by itself.',
        attention: true,
      };
    case 'setUpLights':
      return {
        title: 'Set up your lights',
        description: 'You add them from your phone, in the web portal.',
        attention: true,
      };
    case 'updating':
      // The service is newer than this app, as during an update.
      return {
        title: 'Updating…',
        description: 'Update this app if this stays.',
        attention: false,
      };
    case 'unreadable':
      return {
        title: 'Couldn’t read settings',
        description: 'Change a setting below to save new ones.',
        attention: true,
      };
    case 'asleep':
      return {
        title:
          situation.power === 'screen_saver'
            ? 'Screen saver is on'
            : 'TV is asleep',
        description: words.wakes,
        attention: false,
      };
    case 'noVideo':
      return {
        title: 'Waiting for video',
        description: 'Choose an input or play something on the TV.',
        attention: false,
      };
    default:
      return unreachable(situation);
  }
}

/** The capture gives no picture: neither is a failure. */
export function noPictureCopy({ picture }: NoPicture, flavor: Flavor): Copy {
  return { ...waitingStatus[picture][flavor], attention: false };
}
