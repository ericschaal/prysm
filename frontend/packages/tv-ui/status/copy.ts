/**
 * The status card's words that both packages use, and the primary
 * button's labels.
 */
import { outputIssueHeadline, type Picture } from '@prysm/application-client';
import {
  isEnabled,
  type Asked,
  type Flavor,
  type State,
} from '@prysm/webos-platform';
import type { Button, PrimaryAction } from './situation';

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
  button?: Button;
  setupLights?: boolean;
}

/** Before a state: waiting for the service's first reply, or no service. */
export const linkCopy: Record<'connecting' | 'missing', Copy> = {
  connecting: {
    title: 'Connecting…',
    description: 'Checking Prysm on this TV.',
    button: 'wait',
    attention: false,
  },
  missing: {
    title: 'Prysm needs setting up',
    description: 'Set up its service on this TV to start.',
    button: 'setup',
    attention: true,
  },
};

export function note(title: string, description: string): Copy {
  return { title, description, attention: false };
}
export function warning(title: string, description: string): Copy {
  return { title, description, attention: true };
}

export function problemCopy(asked: Asked, title: string, flavor: Flavor): Copy {
  if (asked.step === 'unanswered')
    return warning(
      outputIssueHeadline('unreachable'),
      flavor === 'full'
        ? 'Prysm stopped trying. Check them, then try again.'
        : 'Prysm stopped trying. Check Prysm, then try again.',
    );
  switch (asked.fix) {
    case 'try_again':
      return warning(title, 'Something went wrong. Try again to restart it.');
    case 'change_settings':
      return warning(
        title,
        flavor === 'full'
          ? 'Change the settings in the web portal.'
          : 'Change the settings in Prysm’s web portal.',
      );
    case 'set_up':
      return linkCopy.missing;
    case 'unsupported':
      return warning(
        'Capture not supported',
        'This TV doesn’t let Prysm capture its picture.',
      );
    case 'restart_tv':
      return warning(
        'Restart the TV',
        'Prysm can’t capture again until the TV restarts.',
      );
  }
}

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

/** How the package describes stopping and waking. */
const packageWords: Record<Flavor, { stopping: string; wakes: string }> = {
  full: {
    stopping: 'Restarting Prysm…',
    wakes: 'Screen sync continues when the TV wakes up.',
  },
  slim: {
    stopping: 'Stopping capture…',
    wakes: 'Capture continues when the TV wakes up.',
  },
};

export function stopping(flavor: Flavor): Copy {
  return note(packageWords[flavor].stopping, 'This takes a moment.');
}

export const setupLights = {
  ...warning(
    'Set up your lights',
    'You add them from your phone, in the web portal.',
  ),
  setupLights: true,
};
export const updating = note('Updating…', 'Update this app if this stays.');

export function tvCopy(state: State): Copy | null {
  if (state.unreadable !== undefined)
    return warning(
      'Couldn’t read settings',
      'Change a setting below to save new ones.',
    );
  if (!isEnabled(state)) return null;
  if (state.power !== 'active')
    return note(
      state.power === 'screen_saver' ? 'Screen saver is on' : 'TV is asleep',
      packageWords[state.flavor].wakes,
    );
  if (state.video === 'unavailable')
    return note(
      'Waiting for video',
      'Choose an input or play something on the TV.',
    );
  return null;
}

export function noPictureCopy(
  picture: Exclude<Picture, 'arriving'>,
  flavor: Flavor,
): Copy {
  return { ...waitingStatus[picture][flavor], attention: false };
}
