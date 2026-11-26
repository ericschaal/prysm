/**
 * The status card: what each package's situation is (`full.ts`, `slim.ts`),
 * in its words (`copy.ts`, `full-copy.ts`, `slim-copy.ts`), with its button.
 */
import { unreachable } from '@prysm/application-client';
import type { State } from '@prysm/webos-platform';
import type { SavedPreferences } from '../preferences';
import { buttons, linkCopy } from './copy';
import { fullButton, fullSituation } from './full';
import { fullCopy } from './full-copy';
import { ready, type PortalFacts, type PrimaryAction } from './situation';
import { slimButton, slimSituation } from './slim';
import { slimCopy } from './slim-copy';

/**
 * What the status card says: one headline and one line of detail. Every
 * detail fits one line of the card (about 52 characters); addresses are
 * never part of it (the portal card shows them).
 */
export interface TvStatus {
  title: string;
  description: string;
  label: string;
  action: PrimaryAction;
  /** Something went wrong (calm amber), not just a wait. */
  attention: boolean;
  /** Frames flow from an awake TV with video. */
  ready: boolean;
  /**
   * The portal has no lights yet: its setup is the one next step, so the
   * screen leads with the portal address and its QR code.
   */
  setupLights: boolean;
}

/**
 * What the status card needs of the service subscription (`Link` in
 * `service.ts`): waiting for the first reply, the service missing (a state
 * kept from before is not shown), or a state received.
 */
export type ServiceLink =
  | { status: 'connecting' | 'missing' }
  | { status: 'live' | 'lost'; state: State };

/**
 * The status card and primary button from the service subscription;
 * `draft` is the preferences as shown (saved, or being saved); `portal` is
 * `null` until the portal's state is known.
 */
export function tvStatus(
  link: ServiceLink,
  draft: SavedPreferences,
  portal: PortalFacts | null = null,
): TvStatus {
  switch (link.status) {
    case 'connecting':
      return {
        ...linkCopy.connecting,
        ...buttons.wait,
        ready: false,
        setupLights: false,
      };
    case 'missing':
      return {
        ...linkCopy.missing,
        ...buttons.setup,
        ready: false,
        setupLights: false,
      };
    case 'live':
    case 'lost':
      break;
    default:
      return unreachable(link);
  }
  const { state } = link;
  if (state.flavor === 'full') {
    const situation = fullSituation(state, portal);
    return {
      ...fullCopy(situation),
      ...buttons[fullButton(state, situation)],
      ready: ready(state),
      setupLights: situation.kind === 'setUpLights',
    };
  }
  const situation = slimSituation(state, portal);
  return {
    ...slimCopy(situation),
    ...buttons[slimButton(state, draft)],
    ready: ready(state),
    setupLights: situation.kind === 'setUpLights',
  };
}
