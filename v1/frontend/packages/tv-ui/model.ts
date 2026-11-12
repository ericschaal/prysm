/** Pure rules of the TV UI: addresses, drafts and the status card. */
import type { Preferences, SlimEncoding, State } from '@prysm/webos-platform';

export type { SlimEncoding };

export const PORTAL_PORT = 8787;

/** The short names of the slim stream encodings (chosen in Prysm's portal). */
const encodingNames: Record<SlimEncoding, string> = {
  h264_hardware: 'Hardware H.264',
  h264_software: 'Software H.264',
  raw: 'Uncompressed',
};

/** The short name of an encoding, as the status card shows it. */
export function encodingName(encoding: SlimEncoding): string {
  return encodingNames[encoding] || encoding;
}

/** Structural equality ignoring key order and `undefined` fields. */
export function equal(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || !a || !b) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const keys = Object.keys(left).filter((key) => left[key] !== undefined);
  if (keys.length !== Object.keys(right).filter((key) => right[key] !== undefined).length)
    return false;
  return keys.every((key) => equal(left[key], right[key]));
}

/** Preferences with every field present, as the supervisor stores them. */
export type SavedPreferences = Required<Preferences>;
export function completePreferences(value: Preferences): SavedPreferences {
  return {
    autostart: value.autostart ?? false,
    instance: value.instance ?? null,
    receiver: value.receiver ?? null,
  };
}

/**
 * The portal origin from what a user typed: a host name or IP becomes
 * `http://host:8787`; a full `http(s)://` origin is kept; a trailing `/` is
 * dropped. Returns an error text for anything that is not an origin.
 */
export function instanceAddress(
  input: string,
): { value: string | null } | { error: string } {
  const text = input.trim().replace(/\/+$/, '');
  if (!text) return { value: null };
  const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(text)
    ? text
    : `http://${text}${/:\d+$/.test(text) && !/^\[[^\]]*\]$/.test(text) ? '' : `:${PORTAL_PORT}`}`;
  if (!/^https?:\/\/(\[[0-9a-f:.]+\]|[^\s/?#@[\]:]+)(:\d{1,5})?$/i.test(withScheme))
    return {
      error: 'Enter an address such as 192.168.1.20 or http://prysm.local:8787.',
    };
  return { value: withScheme.replace(/^[a-z]+/i, (scheme) => scheme.toLowerCase()) };
}

/** The host of an origin, with IPv6 brackets. */
export function hostOf(origin: string): string | null {
  const match = /^https?:\/\/(\[[^\]]+\]|[^/:]+)/i.exec(origin);
  return match ? match[1]! : null;
}

const ipv4 = /^(25[0-5]|2[0-4]\d|1?\d?\d)(\.(25[0-5]|2[0-4]\d|1?\d?\d)){3}$/;
/** Receivers are socket addresses: an IP literal is needed, not a name. */
export const isIpHost = (host: string) =>
  ipv4.test(host) || /^\[[0-9a-f:.]+\]$/i.test(host);

/** A receiver address `ip:port`, checked the way the supervisor parses it. */
export function receiverAddress(
  input: string,
): { value: string | null } | { error: string } {
  const text = input.trim();
  if (!text) return { value: null };
  const match = /^(.+):(\d{1,5})$/.exec(text);
  const port = match ? Number(match[2]) : 0;
  if (!match || !isIpHost(match[1]!) || port < 1 || port > 65535)
    return { error: 'Enter an IP address and port, such as 192.168.1.20:7380.' };
  return { value: text };
}

/** The port a remote Prysm receives TV frames on, from its capture section. */
export function listenPort(capture: unknown): number | null {
  const section = capture as { kind?: unknown; listen?: unknown } | undefined;
  if (section?.kind !== 'remote' || typeof section.listen !== 'string')
    return null;
  const match = /:(\d{1,5})$/.exec(section.listen);
  const port = match ? Number(match[1]) : 0;
  return port >= 1 && port <= 65535 ? port : null;
}

/** Where frames should go for a Prysm at `instance` that listens on `port`. */
export function derivedReceiver(
  instance: string | null | undefined,
  port: number | null,
): string | null {
  const host = instance ? hostOf(instance) : null;
  return host && port && isIpHost(host) ? `${host}:${port}` : null;
}

/**
 * The full worker binds `0.0.0.0`; the TV app reaches it on the loopback
 * address with the advertised port, which stays right when the LAN address
 * changes.
 */
export function localPortal(portal: string | null | undefined): string | null {
  if (!portal) return null;
  const match = /^https?:\/\/(?:\[[^\]]+\]|[^/:]+)(?::(\d+))?/i.exec(portal);
  if (!match) return null;
  return `http://127.0.0.1:${match[1] ?? '80'}`;
}

/** The first line of a failure, short enough for the status card. */
export function failureText(failure: string): string {
  const line = failure.split('\n')[0]!.trim();
  return line.length > 180 ? `${line.slice(0, 177)}…` : line;
}

export type PrimaryAction =
  | 'none'
  | 'setup'
  | 'disable'
  | 'address'
  | 'save_enable'
  | 'enable';

export interface TvStatus {
  title: string;
  description: string;
  label: string;
  action: PrimaryAction;
  attention: boolean;
  /** Frames flow from an awake TV with video. */
  ready: boolean;
}

/** What the TV UI knows from the portal that drives the lights. */
export interface PortalFacts {
  /** The portal's settings have an output. */
  lights: boolean;
  /** Where people open the portal, if known. */
  address: string | null;
}

/**
 * The status card and primary button from the supervisor's state. `online`
 * is `null` until the first reply; `draft` is the preferences being edited;
 * `portal` is `null` until the portal's state is known.
 */
export function tvStatus(
  online: boolean | null,
  state: State | null,
  draft: SavedPreferences,
  dirty: boolean,
  portal: PortalFacts | null = null,
): TvStatus {
  if (online === null || (online && !state))
    return {
      title: 'Connecting…',
      description: 'Checking the Prysm service on this TV.',
      label: 'Connecting…',
      action: 'none',
      attention: false,
      ready: false,
    };
  if (!online || !state)
    return {
      title: 'TV service unavailable',
      description: 'Set up the background service so Prysm can capture this TV.',
      label: 'Set up service',
      action: 'setup',
      attention: true,
      ready: false,
    };
  const full = state.flavor === 'full';
  const { enabled } = state;
  const failure = state.failure ?? null;
  // Slim: the receiver is unavailable; the worker keeps trying, not a failure.
  const waiting = !!failure && state.failure_kind === 'receiver';
  let title: string;
  let description: string;
  let attention = false;
  // Slim: the TV cannot produce the stream Prysm asked for. The encoding is
  // Prysm's setting, so the fix is in its portal, not here.
  const encodingHint =
    !full && !!failure && state.failure_kind === 'encoder'
      ? ' Choose another stream encoding under Capture in Prysm’s web portal.'
      : '';
  if (state.worker === 'stopping') {
    title = full ? 'Restarting Prysm…' : 'Stopping capture…';
    description = 'Finishing the current capture session.';
  } else if (state.worker === 'retrying') {
    title = 'Capture needs attention';
    description = `${failure ? `${failureText(failure)}. ` : ''}Prysm tries again automatically.${encodingHint}`;
    attention = true;
  } else if (enabled && state.power !== 'active') {
    title = 'Waiting for TV';
    description = full
      ? 'Screen sync resumes when the TV wakes up.'
      : 'Capture resumes when the TV wakes up.';
  } else if (enabled && state.video === 'unavailable') {
    title = 'Waiting for video';
    description = `Select an HDMI input or play a video on the TV.${full ? FULL_RELEASED : ''}`;
  } else if (full) {
    if (!enabled) {
      title = 'Lights off';
      description = 'Start screen sync here, or use the web portal for your lights.';
    } else if (state.lighting === 'device_native') {
      title = 'Light effects on';
      description = 'Your lights run their own effects.';
    } else if (state.capturing) {
      title = 'Screen sync is on';
      description = 'This TV is processing the picture and sending colours to your lights.';
    } else if (state.portal && state.worker === 'running') {
      // Lighting is on but no frames arrive: the lights are released to WLED.
      title = 'Waiting for video';
      description = `Screen sync starts when a picture arrives.${FULL_RELEASED}`;
    } else {
      title = 'Starting Prysm…';
      description = 'Waiting for the web portal to open.';
    }
  } else if (!state.preferences.receiver) {
    title = 'Not set up';
    description = draft.receiver
      ? `Save to send this TV’s picture to ${draft.receiver}.`
      : 'Add the address of the Prysm on your computer.';
  } else if (!enabled) {
    title = 'Capture off';
    description = 'Start capture to send this TV’s picture to Prysm.';
  } else if (waiting) {
    title = 'Waiting for Prysm';
    description = `This TV connects to ${state.preferences.receiver} again every second. Check that Prysm is running with Remote capture.`;
  } else if (state.capturing) {
    title = 'Sending the picture';
    description = 'Your TV picture is going to Prysm.';
  } else {
    title = 'Starting capture…';
    description = 'Waiting for the first picture.';
  }
  // Lights are set up in the portal (its first-run setup), never here.
  const linked = full || !!state.preferences.receiver;
  const noLights = !!portal && !portal.lights && linked;
  if (noLights && (!failure || waiting) && state.worker !== 'stopping' && state.worker !== 'retrying') {
    title = 'Set up your lights';
    description = `Open ${portal.address ?? 'Prysm’s web portal'} on your phone or computer to set up your lights.`;
    attention = true;
  }
  if (failure && !waiting && state.worker !== 'retrying') {
    title = 'Capture needs attention';
    description = `${failureText(failure)}.${encodingHint} Open Service options for diagnostics.`;
    attention = true;
  }

  let label: string;
  let action: PrimaryAction;
  if (enabled) {
    label = full ? 'Turn lights off' : 'Stop capture';
    action = 'disable';
  } else if (full) {
    label = 'Start screen sync';
    // Screen sync cannot start without lights.
    action = noLights ? 'none' : 'enable';
  } else if (!draft.receiver) {
    label = 'Set Prysm address';
    action = 'address';
  } else if (dirty) {
    label = 'Save and start capture';
    action = 'save_enable';
  } else {
    label = 'Start capture';
    action = 'enable';
  }
  return {
    title,
    description,
    label,
    action,
    attention,
    ready: state.capturing && state.power === 'active' && state.video !== 'unavailable',
  };
}

const FULL_RELEASED = ' Meanwhile your lights show their own WLED effects.';

const powerNames = {
  unknown: 'unknown',
  active: 'on',
  screen_saver: 'screen saver',
  asleep: 'standby',
} as const;
const phaseNames = {
  stopped: 'stopped',
  running: 'running',
  stopping: 'stopping',
  retrying: 'waiting to restart after a failure',
} as const;

/** The Diagnostics text. */
export function diagnostics(state: State | null): string {
  if (!state) return 'Service unavailable.';
  const lines = [
    `Package: ${state.flavor === 'full' ? 'Prysm (lights from this TV)' : 'Prysm capture (lights from a computer)'}`,
    `TV power: ${powerNames[state.power]}`,
    `Video: ${state.video}`,
    `Enabled: ${state.enabled ? 'yes' : 'no'}`,
    `Background process: ${phaseNames[state.worker]}`,
    `Capturing: ${state.capturing ? 'yes' : 'no'}`,
  ];
  if (state.flavor === 'slim')
    lines.push(
      `Frames go to: ${state.preferences.receiver ?? 'not set'}`,
      `Stream encoding: ${state.encoding ? `${encodingName(state.encoding)} in use` : 'none in use'}`,
    );
  else lines.push(`Web portal: ${state.portal ?? 'not open'}`);
  lines.push('', state.failure ?? 'No errors reported.');
  if (state.log.length) lines.push('', 'Recent messages:', ...state.log);
  return lines.join('\n');
}
