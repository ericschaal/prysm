/** Pure rules of the TV UI: addresses, drafts, the status card and the screen. */
import {
  pictureOf,
  plainFailure,
  REFUSED_VIDEO,
  unreachable,
  unusableSettings,
  type LightingSituation,
  type Picture,
} from '@prysm/application-client';
import {
  captureFps,
  fullPortal,
  isCapturing,
  isEnabled,
  isPaused,
  isRefused,
  slimEncoding,
  type Flavor,
  type LightingTarget,
  type Preferences,
  type StreamEncoding,
  type State,
  type Worker,
} from '@prysm/webos-platform';
import type { PatternPlayer } from './patterns';
import type { PortalView } from './portal-connection';

export type { StreamEncoding };

export const PORTAL_PORT = 8787;

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

/** Preferences with every field present (no address: `null`), as the app edits them. */
export type SavedPreferences = {
  autostart: boolean;
  prysm_address: string | null;
};
export function completePreferences(value: Preferences): SavedPreferences {
  return {
    autostart: value.autostart ?? false,
    prysm_address: value.prysm_address ?? null,
  };
}

/** `saved` as the service takes it: no address is omitted. */
export function sentPreferences(saved: SavedPreferences): Preferences {
  const { autostart, prysm_address } = saved;
  return prysm_address === null ? { autostart } : { autostart, prysm_address };
}

/**
 * The portal origin from what a user typed: a host name or IP becomes
 * `http://host:8787`; a full `http(s)://` origin is kept; a trailing `/` is
 * dropped. Returns an error text for anything that is not an origin.
 */
export function prysmAddress(
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

/**
 * The primary button is about the service on this TV only; what the lights
 * do is chosen with the lighting modes. Full package: set up, or try again
 * after a failure, else no button. Slim: capture on or off.
 */
export type PrimaryAction =
  'none' | 'setup' | 'retry' | 'disable' | 'address' | 'enable';

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

/** What the TV UI knows from the portal that drives the lights. */
export interface PortalFacts {
  /** The portal's settings have an output. */
  lights: boolean;
  /** Where people open the portal, if known. */
  address: string | null;
  /**
   * What the lights do by the portal's own state: the full package's
   * headline follows it, so it never disagrees with the lights card.
   */
  lighting?: LightingSituation;
}

/** The target a running or starting situation was asked for. */
function targetOf(
  lighting: Exclude<
    LightingSituation,
    { kind: 'faulted' | 'failed' | 'unknown' }
  >,
): LightingTarget {
  switch (lighting.kind) {
    case 'off':
      return 'off';
    case 'sync':
      return 'screen_sync';
    case 'effects':
      return 'effects';
    case 'starting':
      return lighting.target;
    default:
      return unreachable(lighting);
  }
}

/**
 * Whole seconds until the supervisor restarts the worker, `elapsedMs` after
 * `worker` arrived (it counts from then: no clock is shared), or `null` when
 * no restart is pending. 0 once it is due.
 */
export function retrySeconds(worker: Worker, elapsedMs: number): number | null {
  if (worker.phase !== 'retrying') return null;
  return Math.max(0, Math.ceil(worker.seconds - elapsedMs / 1000));
}

/** The capture rate the TV reports, for the status card; `null` when none. */
export function captureRate(state: State | null): string | null {
  const fps = state && captureFps(state);
  if (fps === null) return null;
  return fps > 0 ? `${fps} frames per second` : 'No frames yet';
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
 * `null` until the portal's state is known. The retry countdown
 * is shown beside the card (see {@link retrySeconds}), not in its words.
 */
export function tvStatus(
  link: ServiceLink,
  draft: SavedPreferences,
  portal: PortalFacts | null = null,
): TvStatus {
  switch (link.status) {
    case 'connecting':
      return {
        title: 'Connecting…',
        description: 'Checking Prysm on this TV.',
        label: 'Please wait…',
        action: 'none',
        attention: false,
        ready: false,
        setupLights: false,
      };
    case 'missing':
      return {
        title: 'Prysm needs setting up',
        description: 'Set up its service on this TV to start.',
        label: 'Set up service',
        action: 'setup',
        attention: true,
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
  const full = state.flavor === 'full';
  const enabled = isEnabled(state);
  const capturing = isCapturing(state);
  const kind = state.failure?.kind ?? null;
  const phase = state.worker.phase;
  // Slim: Prysm is not receiving; the worker keeps trying, not a failure.
  const waiting = kind === 'receiver';
  // The saved preferences could not be read: the service runs with the
  // defaults, and saving a setting replaces the file. Nothing stopped.
  const unreadable = kind === 'preferences';
  const failed = !!state.failure && !waiting && !unreadable;
  // Full: the worker reports its session's failure, so while the portal's
  // situation is known it says what the failure means (off with a switch
  // off that failed is still off). Before the portal answers, the failure
  // is said only for a mode, so lights that are off never flicker as
  // stopped; Luna does not say whose failure it is, so one of the
  // supervisor's own waits for the portal too. A restart being retried is
  // the supervisor's own.
  const lighting = full ? portal?.lighting : undefined;
  const asksMode =
    state.flavor === 'full' &&
    (state.target === 'screen_sync' || state.target === 'effects');
  const workerFailed =
    failed && (!full || phase === 'retrying' || (!lighting && asksMode));
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
  } else if (failed && !full && kind === 'version') {
    // Prysm and this app speak different stream versions; the TV keeps
    // checking (every second), so updating the older side is enough.
    title = 'Update Prysm or TV app';
    description = 'Update the older of Prysm and this app.';
    attention = true;
  } else if (failed && !full && kind === 'replaced') {
    // Another TV took the stream over; this one stopped until capture is
    // started again, so the two never take it from each other in turn.
    title = 'Another TV took over';
    description = 'Stop and start capture to send from this TV.';
    attention = true;
  } else if (failed && !full && kind === 'receiver_failed') {
    // Prysm's lighting failed with this TV's stream (no decoder, a broken
    // keyframe); the TV keeps trying, and Prysm's portal says why.
    title = 'Prysm stopped receiving';
    description = 'Its web portal on your computer says why.';
    attention = true;
  } else if (workerFailed) {
    title = stopped;
    if (phase === 'retrying')
      description = 'Something went wrong. Prysm tries again by itself.';
    else if (full) description = 'Try again, or choose another mode.';
    else description = 'Stop and start capture again to retry.';
    attention = true;
  } else if (phase === 'unknown') {
    // The service is newer than this app, as during an update; a failure
    // it reports is said first.
    title = 'Updating…';
    description = 'Update this app if this stays.';
    // Full: a faulted or failed session comes before the TV's power and
    // video. It is what needs the user (restart Prysm, try again, another
    // mode); waking the TV or a picture would not end it, and saying
    // "Waiting for video" over it would hide it. Lights that are fine
    // (off, effects, screen sync) let power and video speak first.
  } else if (lighting?.kind === 'faulted') {
    title = 'Prysm stopped responding';
    description = 'Restart Prysm to recover.';
    attention = true;
  } else if (lighting?.kind === 'failed') {
    // The lights card says what failed; its modes try again.
    if (lighting.target === 'effects') title = 'WLED effects stopped';
    else title = lighting.fromEffects ? 'Screen sync couldn’t start' : stopped;
    // Trying again cannot help settings Prysm can't use.
    description = unusableSettings(lighting.failure)
      ? 'Change the settings in the web portal.'
      : 'Try again, or choose another mode.';
    attention = true;
  } else if (unreadable) {
    // After every failure of the lights or the worker, which need the
    // user first; before power and video, which do not.
    title = 'Couldn’t read settings';
    description = 'Change a setting below to save new ones.';
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
  } else if (state.flavor === 'full') {
    if (
      lighting?.kind === 'unknown' ||
      (!lighting && state.target === 'unknown')
    ) {
      title = 'Prysm is newer than this app';
      description = 'Update the app to see what the lights do.';
    } else {
      // The supervisor hears of a change made in the portal a moment later.
      const target = lighting ? targetOf(lighting) : state.target;
      // The portal's measure; it alone says why no picture arrives.
      const picture =
        lighting?.kind === 'sync' && lighting.input
          ? pictureOf(lighting.input)
          : undefined;
      // Without the portal's measure yet, the TV's own word.
      const flowing = lighting
        ? lighting.kind === 'sync' &&
          (picture ? picture === 'arriving' : capturing)
        : capturing;
      if (target === 'off') {
        title = 'Lights off';
        description = 'Choose Screen sync: the lights follow the picture.';
        // Still off, but its failure is said, as the lights card says it:
        // settings that do not validate, or a switch off that failed.
        if (lighting?.kind === 'off' && lighting.failure) {
          description = plainFailure(lighting.failure, 'off', 'WLED effects');
          attention = true;
        }
      } else if (target === 'effects') {
        title = 'WLED effects on';
        description = 'Your lights run their own effects.';
      } else if (flowing) {
        title = 'Screen sync on';
        description = 'Your lights follow the picture.';
      } else if (picture === 'refused') {
        ({ title, description } = waitingStatus.refused.full);
      } else if (fullPortal(state) && phase === 'running') {
        // Lighting is on but no frames arrive: the lights are released to WLED.
        ({ title, description } = waitingStatus.quiet.full);
      } else {
        title = 'Starting screen sync…';
        description = 'This takes a few seconds.';
      }
    }
  } else if (!state.preferences.prysm_address) {
    title = 'Set up this TV';
    description = 'Add the address of Prysm on your computer.';
  } else if (!enabled) {
    title = 'Capture off';
    description = 'Start capture to send the picture to Prysm.';
  } else if (isPaused(state)) {
    // Not a failure: lighting is off on the computer, so the TV stops capturing.
    title = 'Paused by Prysm';
    description = 'It continues when Prysm turns the lights on.';
  } else if (waiting) {
    title = 'Waiting for Prysm';
    description = 'Check that Prysm is running on your computer.';
  } else if (isRefused(state)) {
    ({ title, description } = waitingStatus.refused.slim);
  } else if (capturing) {
    title = 'Sending the picture';
    description = 'Prysm on your computer lights up with it.';
  } else {
    ({ title, description } = waitingStatus.quiet.slim);
  }
  // Lights are set up in the portal (its first-run setup), never here.
  const linked = full || !!state.preferences.prysm_address;
  const noLights = !!portal && !portal.lights && linked;
  const setupLights = noLights && !workerFailed && phase !== 'stopping';
  if (setupLights) {
    title = 'Set up your lights';
    description = 'You add them from your phone, in the web portal.';
    attention = true;
  }

  // Full: the modes start and stop the lights, so the button only restarts
  // after a failure the supervisor does not retry by itself.
  let label = '';
  let action: PrimaryAction = 'none';
  if (phase === 'unknown') {
    // Nothing to start or stop in a phase this app cannot read.
  } else if (phase === 'stopping') {
    if (!full) label = 'Please wait…';
  } else if (full) {
    // Again the target that failed: only a mode, and only while the
    // portal's situation, when known, is that failure.
    const mode = state.target === 'screen_sync' || state.target === 'effects';
    if (
      failed &&
      phase !== 'retrying' &&
      mode &&
      (!lighting ||
        (lighting.kind === 'failed' && !unusableSettings(lighting.failure)))
    ) {
      label = 'Try again';
      action = 'retry';
    }
  } else if (enabled) {
    label = 'Stop capture';
    action = 'disable';
  } else if (!draft.prysm_address) {
    label = 'Add Prysm address';
    action = 'address';
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
      capturing && state.power === 'active' && state.video !== 'unavailable',
    setupLights,
  };
}

/** The main screen around the status card, from the status and what the app knows. */
export interface TvScreen {
  title: string;
  description: string;
  attention: boolean;
  /** The quiet line under the detail: a restart's countdown, or the capture rate. */
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
 * button, and which panels show. `retryLeft` is the countdown to a restart
 * (see {@link retrySeconds}), `null` when none is shown; `changing` is a
 * command on its way.
 */
export function tvScreen({
  flavor,
  status,
  state,
  changing,
  portalAddress,
  retryLeft,
}: {
  flavor: Flavor;
  status: TvStatus;
  state: State | null;
  changing: boolean;
  portalAddress: string | null;
  retryLeft: number | null;
}): TvScreen {
  const slim = flavor === 'slim';
  const mismatch = !!state && state.flavor !== flavor;
  // One quiet line under the detail, always kept: the countdown to a
  // restart, or while frames flow their rate (and the slim encoding).
  let meta = '';
  if (retryLeft !== null)
    meta = retryLeft ? `Trying again in ${retryLeft} s` : 'Trying again now…';
  else if (state && isCapturing(state) && !mismatch) {
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
  unknown: 'unknown (newer than this app)',
} as const;

function capturingText(state: State) {
  if (isCapturing(state)) return 'yes';
  if (isPaused(state))
    return 'no, paused by Prysm (lights off on the computer)';
  if (isRefused(state)) return `no. ${REFUSED_VIDEO}`;
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
    `Enabled: ${isEnabled(state) ? 'yes' : 'no'}`,
    `Background process: ${phaseNames[state.worker.phase]}`,
    `Capturing: ${capturingText(state)}`,
    `Capture rate: ${captureRate(state) ?? 'not measured'}`,
  ];
  if (state.flavor === 'slim') {
    const encoding = slimEncoding(state);
    lines.push(
      `Prysm address: ${state.preferences.prysm_address ?? 'not set'}`,
      `Stream encoding: ${encoding ? `${encodingName(encoding)} in use` : 'none in use'}`,
    );
  } else lines.push(`Web portal: ${fullPortal(state) ?? 'not open'}`);
  lines.push('', state.failure?.message ?? 'No errors reported.');
  if (problem) lines.push('', `Last command failed: ${problem}`);
  if (state.log.length) lines.push('', 'Recent messages:', ...state.log);
  return lines.join('\n');
}

/** An address as people read and type it: no `http://`, no trailing slash. */
export const shown = (address: string) =>
  address.replace(/^http:\/\//i, '').replace(/\/+$/, '');

/** The portal page the phone opens, from the portal address the TV shows. */
export const calibrationUrl = (portal: string) =>
  `${portal.replace(/\/+$/, '')}/#/calibrate`;

/**
 * Why the lights cannot be calibrated from this TV's picture, or `null`.
 * `tv` is the supervisor's state; `address` the slim package's Prysm address.
 */
export function calibrationBlocker(
  flavor: Flavor,
  player: PatternPlayer,
  portal: PortalView,
  tv: State | null,
  address: string | null,
): string | null {
  if (!player.supported || !portal.supported) return 'Needs a newer TV';
  // Before the service answers, the address is not known yet.
  if (!tv) return 'Waiting for Prysm';
  if (flavor === 'slim' && !address) return 'Add the address first';
  if (flavor === 'slim' && !isEnabled(tv)) return 'Start capture first';
  const { snapshot } = portal;
  if (!snapshot) return 'Waiting for Prysm';
  if (!snapshot.settings.capture || !snapshot.settings.output)
    return 'Set up lights first';
  if (flavor === 'slim' && snapshot.settings.capture.kind !== 'remote')
    return 'Uses another picture';
  return null;
}
