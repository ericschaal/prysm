/** The main screen around the status card: its meta line and panels. */
import {
  captureFps,
  isCapturing,
  slimEncoding,
  type Flavor,
  type State,
  type StreamEncoding,
} from '@prysm/webos-platform';
import type { TvStatus } from './status';

/** The short names of the slim stream encodings (chosen in Prysm's portal). */
const encodingNames: Record<StreamEncoding, string> = {
  h264_hardware: 'Hardware H.264',
  h264_software: 'Software H.264',
  raw: 'Uncompressed',
};

/**
 * The short name of an encoding, as the status card shows it; one newer than
 * this app (the service passes it on) is shown as the service names it.
 */
export function encodingName(encoding: StreamEncoding): string {
  return encodingNames[encoding] || encoding;
}

/** The capture rate the TV reports, for the status card; `null` when none. */
export function captureRate(state: State | null): string | null {
  const fps = state && captureFps(state);
  if (fps === null) return null;
  return fps > 0 ? `${fps} frames per second` : 'No frames yet';
}

/** The main screen around the status card, from the status and what the app knows. */
export interface TvScreen {
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
  status,
  state,
  changing,
  portalAddress,
}: {
  flavor: Flavor;
  status: TvStatus;
  state: State | null;
  changing: boolean;
  portalAddress: string | null;
}): TvScreen {
  const slim = flavor === 'slim';
  const mismatch = !!state && state.flavor !== flavor;
  // One quiet line under the detail, always kept: while frames flow their
  // rate (and the slim encoding).
  let meta = '';
  if (state && isCapturing(state) && !mismatch) {
    const encoding = slimEncoding(state);
    meta = [captureRate(state), encoding ? encodingName(encoding) : null]
      .filter(Boolean)
      .join(' · ');
  }
  // Full: no button while there is nothing for the service to do; the
  // lighting modes are the one way to choose what the lights do. Its place
  // under the status lines (where slim has its button) stays reserved, so the
  // lights card never moves when it comes (Try again, Set up service). Slim
  // keeps its button while capture stops ("Please wait…"). Full: never only
  // because a setting is being saved, or a "Please wait…" button would fill
  // the empty place for the length of the call.
  const showPrimary =
    status.action !== 'none' || (slim && (changing || !!status.label));
  const setupFirst = status.setupLights && !mismatch && !!portalAddress;
  return {
    title: mismatch ? 'Reinstall Prysm' : status.title,
    description: mismatch
      ? 'The app and its TV service don’t match.'
      : status.description,
    attention: status.attention || mismatch,
    meta,
    showPrimary,
    label: changing ? 'Please wait…' : status.label,
    waiting: changing || status.action === 'none',
    mismatch,
    setupFirst,
    // The full package's portal address arrives with the worker; its panel
    // keeps its place meanwhile, empty (nothing to scan yet), so Settings
    // does not move down.
    portalPanel: !setupFirst && (!!portalAddress || !state || !slim),
  };
}
