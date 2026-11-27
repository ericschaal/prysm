/** The main screen around the status card: its meta line and panels. */
import {
  captureFps,
  isCapturing,
  slimEncoding,
  type Flavor,
  type State,
} from '@prysm/webos-platform';
import type { SavedPreferences } from './preferences';
import { lastState, type Link } from './service';
import { cardCopy } from './status';
import { buttons } from './status/copy';
import {
  ready,
  type PortalFacts,
  type PrimaryAction,
} from './status/situation';

/** The short names of the slim stream encodings (chosen in Prysm's portal). */
const encodingNames = new Map<string, string>([
  ['h264_hardware', 'Hardware H.264'],
  ['h264_software', 'Software H.264'],
  ['raw', 'Uncompressed'],
]);

/**
 * The short name of an encoding, as the status card shows it; one newer than
 * this app (the service passes it on) is shown as the service names it.
 */
export function encodingName(encoding: string): string {
  return encodingNames.get(encoding) ?? encoding;
}

/** The capture rate the TV reports, for the status card; `null` when none. */
export function captureRate(state: State | null): string | null {
  const fps = state && captureFps(state);
  if (fps === null) return null;
  return fps > 0 ? `${fps} frames per second` : 'No frames yet';
}

interface TvScreen {
  ready: boolean;
  action: PrimaryAction;
  title: string;
  description: string;
  attention: boolean;
  /** The quiet line under the detail: the capture rate while frames flow. */
  meta: string;
  /** The primary button is shown, with this label. */
  showPrimary: boolean;
  label: string;
  /** It waits: it keeps focus (a disabled button would drop it) and OK does nothing. */
  waiting: boolean;
  /** The app and its TV service are different packages: nothing to press. */
  mismatch: boolean;
  /** The portal's QR code takes the place of the lights card: set up lights first. */
  setupFirst: boolean;
  /** The portal address panel beside the settings. */
  portalPanel: boolean;
}

/**
 * The main screen: the status card's words, its one line of meta and its
 * button, and which panels show. `changing` is a command on its way.
 */
export function tvScreen({
  flavor,
  link,
  draft,
  portal,
  changing,
  portalAddress,
}: {
  flavor: Flavor;
  link: Link;
  draft: SavedPreferences;
  portal: PortalFacts | null;
  changing: boolean;
  portalAddress: string | null;
}): TvScreen {
  const copy = cardCopy(link, draft, portal);
  const { action, label } = buttons[copy.button ?? 'none'];
  // Missing service keeps its last state for package checks and capture details.
  const state = lastState(link);
  const slim = flavor === 'slim';
  const mismatch = !!state && state.flavor !== flavor;
  let meta = '';
  if (state && isCapturing(state) && !mismatch) {
    const encoding = slimEncoding(state);
    meta = [captureRate(state), encoding ? encodingName(encoding) : null]
      .filter(Boolean)
      .join(' · ');
  }
  const showPrimary = action !== 'none' || (slim && (changing || !!label));
  const setupFirst = !!copy.setupLights && !mismatch && !!portalAddress;
  return {
    ready: !!state && link.status !== 'missing' && ready(state),
    action,
    title: mismatch ? 'Reinstall Prysm' : copy.title,
    description: mismatch
      ? 'The app and its TV service don’t match.'
      : copy.description,
    attention: copy.attention || mismatch,
    meta,
    showPrimary,
    label: changing ? 'Please wait…' : label,
    waiting: changing || action === 'none',
    mismatch,
    setupFirst,
    portalPanel: !setupFirst && (!!portalAddress || !state || !slim),
  };
}
