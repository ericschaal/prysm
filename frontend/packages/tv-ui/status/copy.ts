/**
 * The status card's words that both packages use, and the primary
 * button's labels.
 */
import { outputIssueHeadline } from '@prysm/application-client';
import {
  isEnabled,
  type Asked,
  type Flavor,
  type State,
} from '@prysm/webos-platform';
import type { Button, PrimaryAction } from './situation';

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

const captureName: Record<Flavor, string> = {
  full: 'Screen sync',
  slim: 'Capture',
};

export function refusedVideo(flavor: Flavor): Copy {
  return note(
    'Video can’t be captured',
    `${captureName[flavor]} continues when the video changes.`,
  );
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
      `${captureName[state.flavor]} continues when the TV wakes up.`,
    );
  if (state.video === 'unavailable')
    return note(
      'Waiting for video',
      'Choose an input or play something on the TV.',
    );
  return null;
}
