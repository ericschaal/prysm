/** Pure rules of the TV UI: addresses, drafts and the status card. */
import type {
  Phase,
  Preferences,
  SlimEncoding,
  State,
} from '@prysm/webos-platform';
import type { PatternPlayer } from './patterns';
import type { PortalView } from './portal-connection';

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

/** Preferences with every field present, as the supervisor stores them. */
export type SavedPreferences = Required<Preferences>;
export function completePreferences(value: Preferences): SavedPreferences {
  return {
    autostart: value.autostart ?? false,
    instance: value.instance ?? null,
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
  if (
    !/^https?:\/\/(\[[0-9a-f:.]+\]|[^\s/?#@[\]:]+)(:\d{1,5})?$/i.test(
      withScheme,
    )
  )
    return {
      error: 'That doesn’t look like an address. Try one like 192.168.1.20.',
    };
  return {
    value: withScheme.replace(/^[a-z]+/i, (scheme) => scheme.toLowerCase()),
  };
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

export type PrimaryAction =
  'none' | 'setup' | 'disable' | 'address' | 'save_enable' | 'enable';

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
}

/** What the TV UI knows from the portal that drives the lights. */
export interface PortalFacts {
  /** The portal's settings have an output. */
  lights: boolean;
  /** Where people open the portal, if known. */
  address: string | null;
}

/**
 * Whole seconds until the supervisor restarts the worker, `elapsedMs` after
 * `phase` arrived (it counts from then: no clock is shared), or `null` when
 * no restart is pending. 0 once it is due.
 */
export function retrySeconds(phase: Phase, elapsedMs: number): number | null {
  if (phase.phase !== 'retrying') return null;
  return Math.max(0, Math.ceil(phase.seconds - elapsedMs / 1000));
}

/** The capture rate the TV reports, for the status card; `null` when none. */
export function captureRate(state: State | null): string | null {
  if (!state || state.fps === null || state.fps === undefined) return null;
  return state.fps > 0 ? `${state.fps} frames per second` : 'No frames yet';
}

/**
 * The status card and primary button from the supervisor's state. `online`
 * is `null` until the first reply; `draft` is the preferences being edited;
 * `portal` is `null` until the portal's state is known. The retry countdown
 * is shown beside the card (see {@link retrySeconds}), not in its words.
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
      description: 'Checking Prysm on this TV.',
      label: 'Please wait…',
      action: 'none',
      attention: false,
      ready: false,
    };
  if (!online || !state)
    return {
      title: 'Prysm needs setting up',
      description: 'Set up its service on this TV to start.',
      label: 'Set up service',
      action: 'setup',
      attention: true,
      ready: false,
    };
  const full = state.flavor === 'full';
  const { enabled } = state;
  const kind = state.failure?.kind ?? null;
  const phase = state.worker.phase;
  // Slim: Prysm is not receiving; the worker keeps trying, not a failure.
  const waiting = kind === 'receiver';
  const failed = !!state.failure && !waiting;
  const stopped = full ? 'Screen sync stopped' : 'Capture stopped';
  let title: string;
  let description: string;
  let attention = false;
  if (phase === 'stopping') {
    title = full ? 'Restarting Prysm…' : 'Stopping capture…';
    description = 'This takes a moment.';
  } else if (failed && !full && kind === 'encoder') {
    // The encoding is Prysm's setting, so the fix is in its portal.
    title = 'Encoding not supported';
    description = 'Choose another encoding in Prysm’s web portal.';
    attention = true;
  } else if (failed) {
    title = stopped;
    if (phase === 'retrying')
      description = 'Something went wrong. Prysm tries again by itself.';
    else if (full) description = 'Turn the lights off and on again to retry.';
    else description = 'Stop and start capture again to retry.';
    attention = true;
  } else if (enabled && state.power !== 'active') {
    title =
      state.power === 'screen_saver' ? 'Screen saver is on' : 'TV is asleep';
    description = full
      ? 'Screen sync continues when the TV wakes up.'
      : 'Capture continues when the TV wakes up.';
  } else if (enabled && state.video === 'unavailable') {
    title = 'Waiting for video';
    description = 'Choose an input or play something on the TV.';
  } else if (full) {
    if (!enabled) {
      title = 'Lights off';
      description = 'Start screen sync and the lights follow the picture.';
    } else if (state.lighting === 'device_native') {
      title = 'WLED effects on';
      description = 'Your lights run their own effects.';
    } else if (state.capturing) {
      title = 'Screen sync on';
      description = 'Your lights follow the picture.';
    } else if (state.portal && phase === 'running') {
      // Lighting is on but no frames arrive: the lights are released to WLED.
      title = 'Waiting for video';
      description = 'Screen sync starts when a picture arrives.';
    } else {
      title = 'Starting screen sync…';
      description = 'This takes a few seconds.';
    }
  } else if (!state.preferences.instance) {
    title = 'Set up this TV';
    description = draft.instance
      ? 'Save to send this TV’s picture to Prysm.'
      : 'Add the address of Prysm on your computer.';
  } else if (!enabled) {
    title = 'Capture off';
    description = 'Start capture to send the picture to Prysm.';
  } else if (state.paused) {
    // Not a failure: lighting is off on the computer, so the TV stops capturing.
    title = 'Paused by Prysm';
    description = 'It continues when Prysm turns the lights on.';
  } else if (waiting) {
    title = 'Waiting for Prysm';
    description = 'Check that Prysm is running on your computer.';
  } else if (state.capturing) {
    title = 'Sending the picture';
    description = 'Prysm on your computer lights up with it.';
  } else {
    title = 'Starting capture…';
    description = 'Waiting for the first picture.';
  }
  // Lights are set up in the portal (its first-run setup), never here.
  const linked = full || !!state.preferences.instance;
  const noLights = !!portal && !portal.lights && linked;
  if (noLights && !failed && phase !== 'stopping') {
    title = 'Set up your lights';
    description = 'Open the web portal on your phone to add them.';
    attention = true;
  }

  let label: string;
  let action: PrimaryAction;
  if (phase === 'stopping') {
    label = 'Please wait…';
    action = 'none';
  } else if (enabled) {
    label = full ? 'Turn lights off' : 'Stop capture';
    action = 'disable';
  } else if (full) {
    label = 'Start screen sync';
    // Screen sync cannot start without lights.
    action = noLights ? 'none' : 'enable';
  } else if (!draft.instance) {
    label = 'Add Prysm address';
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
    ready:
      state.capturing &&
      state.power === 'active' &&
      state.video !== 'unavailable',
  };
}

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

function capturingText(state: State) {
  if (state.capturing) return 'yes';
  if (state.paused) return 'no, paused by Prysm (lights off on the computer)';
  return 'no';
}

/**
 * The Diagnostics text: the one place for technical words. `problem` is the
 * last command's error as the service gave it.
 */
export function diagnostics(state: State | null, problem = ''): string {
  if (!state)
    return problem ? `Service unavailable: ${problem}` : 'Service unavailable.';
  const lines = [
    `Package: ${state.flavor === 'full' ? 'Prysm (lights from this TV)' : 'Prysm capture (lights from a computer)'}`,
    `TV power: ${powerNames[state.power]}`,
    `Video: ${state.video}`,
    `Enabled: ${state.enabled ? 'yes' : 'no'}`,
    `Background process: ${phaseNames[state.worker.phase]}`,
    `Capturing: ${capturingText(state)}`,
    `Capture rate: ${captureRate(state) ?? 'not measured'}`,
  ];
  if (state.flavor === 'slim')
    lines.push(
      `Prysm address: ${state.preferences.instance ?? 'not set'}`,
      `Stream encoding: ${state.encoding ? `${encodingName(state.encoding)} in use` : 'none in use'}`,
    );
  else lines.push(`Web portal: ${state.portal ?? 'not open'}`);
  lines.push('', state.failure?.message ?? 'No errors reported.');
  if (problem) lines.push('', `Last command failed: ${problem}`);
  if (state.log.length) lines.push('', 'Recent messages:', ...state.log);
  return lines.join('\n');
}

/** The portal page the phone opens, from the portal address the TV shows. */
export const calibrationUrl = (portal: string) =>
  `${portal.replace(/\/+$/, '')}/#/calibrate`;

/**
 * Why the lights cannot be calibrated from this TV's picture, or `null`.
 * `tv` is the supervisor's state; `instance` the slim package's Prysm address.
 */
export function calibrationBlocker(
  flavor: 'full' | 'slim',
  player: PatternPlayer,
  portal: PortalView,
  tv: State | null,
  instance: string | null,
): string | null {
  if (!player.supported || !portal.supported) return 'Needs a newer TV';
  if (flavor === 'slim' && !instance) return 'Add the address first';
  if (flavor === 'slim' && !tv?.enabled) return 'Start capture first';
  const { snapshot } = portal;
  if (!snapshot) return 'Waiting for Prysm';
  if (!snapshot.settings.capture || !snapshot.settings.output)
    return 'Set up lights first';
  const kind = (snapshot.settings.capture as { kind?: string }).kind;
  if (flavor === 'slim' && kind !== 'remote') return 'Uses another picture';
  return null;
}
