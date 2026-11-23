import type {
  CalibrationPattern,
  Candidate,
  CaptureSettings,
  ColorsEvent,
  Failure,
  FailureSection,
  Input,
  LightingFailure,
  LightingTarget,
  Mode,
  Modes,
  OutputIssue,
  Pattern,
  PreviewEvent,
  Session,
  Settings as GeneratedSettings,
  State,
} from '@prysm/contracts/application';

export type {
  CalibrationPattern,
  Candidate,
  CaptureSettings,
  ColorsEvent,
  Failure,
  FailureSection,
  LightingFailure,
  LightingTarget,
  Mode,
  Modes,
  OutputIssue,
  Pattern,
  PreviewEvent,
  Session,
};
export { same } from './same';

/**
 * How long a stream of the portal (state, colours) waits before it
 * reconnects after `attempt` failures in a row since it last received
 * (0 for the first): 250 ms doubling to 5 s. One policy for every stream of
 * one portal; the TV app's `STALE_MS` is derived from it.
 */
export const reconnectDelayMs = (attempt: number) =>
  Math.min(5000, 250 * 2 ** attempt);

/**
 * Why the lights are not showing screen sync, for people. Prysm keeps
 * sending and does not switch the lights back; they follow again as soon as
 * they take its colours. An issue newer than this page gets the general line.
 */
export function describeOutputIssue(issue: OutputIssue): string {
  switch (issue) {
    case 'off':
      return 'The lights were switched off on the device. Switch them on there to see screen sync again.';
    case 'overridden':
      return 'Live override is on in the lights’ own app, so they ignore screen sync. Turn it off there.';
    case 'not_live':
      return 'The lights are not taking Prysm’s colours. In WLED, check that Sync Interfaces receives UDP realtime (DDP).';
    case 'other_sender':
      return 'Another program is sending colours to the lights. Stop it to see screen sync alone.';
    case 'unreachable':
      return 'The lights do not answer. Check that they are powered and on the network.';
    default:
      return 'The lights are not showing screen sync.';
  }
}
export type {
  LedColor,
  LedColors,
  PreviewFrame,
} from '@prysm/contracts/application';

/** What the lights show while on: screen sync or the output's own effects. */
export type LightingMode = Exclude<LightingTarget, 'off'>;

/**
 * A target as this page reads it: `unknown` is one newer than this page,
 * which selects no mode. Choosing a known one still works.
 */
export type TargetReading = LightingTarget | 'unknown';

/**
 * A session as this page reads it: `unknown` is a state newer than this
 * page; a failure or message it carries is kept, to be shown.
 */
export type SessionReading =
  Session | { state: 'unknown'; failure?: LightingFailure; message?: string };

/**
 * For the `default` of a switch that covers every value: the compiler
 * refuses it while a value is left out.
 */
export function unreachable(value: never): never {
  throw new Error(`Unexpected value: ${JSON.stringify(value)}`);
}

/** The failure `session` carries, if any. */
function sessionFailure(session: SessionReading): LightingFailure | null {
  return 'failure' in session ? (session.failure ?? null) : null;
}

/**
 * The side of the path a failure came from: typed for adapter failures,
 * else the section of the setting it names.
 */
export function failureSide(
  failure: Failure | LightingFailure,
): FailureSection | null {
  // Only the sides this page knows: any other value reads as absent.
  const typed = 'section' in failure ? failure.section : undefined;
  if (typed === 'capture' || typed === 'output') return typed;
  const named = 'field' in failure ? failure.field?.split('.')[0] : undefined;
  return named === 'capture' || named === 'output' ? named : null;
}

/**
 * What the lights are doing, decided once from the requested target and the
 * session (with the failure it carries); every view of the lights shows
 * this. The rule is ordered, so every pair lands on exactly one:
 *
 * - `faulted`: the session stopped responding; nothing works until a
 *   restart. `message` says what did not finish.
 * - `unknown`: the target or the session state is newer than this page.
 * - `off`: the lights were asked off (the session may still be stopping).
 *   With the lights off a stopped session's failure still stays said:
 *   settings that do not validate, or a switch off that failed.
 * - `sync`: screen sync runs; `input` says whether frames arrive, absent
 *   until measured. It never carries a failure.
 * - `effects`: the output's own effects run; `failure` is saved settings
 *   that do not validate.
 * - `failed`: the mode asked for is not running and the session says why;
 *   `at` is the side that failed, when known. `fromEffects`: a start of
 *   screen sync that failed from the lights' own effects; `effectsOn`:
 *   they still show, which is not said when the lights themselves failed.
 * - `starting`: the mode asked for is not running yet (also while switching
 *   from the other one).
 */
export type LightingSituation =
  | { kind: 'faulted'; message: string }
  | { kind: 'unknown'; failure: LightingFailure | null; message: string | null }
  | { kind: 'off'; failure: LightingFailure | null }
  | {
      kind: 'failed';
      target: LightingMode;
      failure: LightingFailure;
      at: FailureSection | null;
      fromEffects: boolean;
      effectsOn: boolean;
    }
  | { kind: 'sync'; input: Input | undefined }
  | { kind: 'effects'; failure: LightingFailure | null }
  | { kind: 'starting'; target: LightingMode };

export function lightingSituation({
  target,
  session,
  input,
}: {
  target: TargetReading;
  session: SessionReading;
  input?: Input;
}): LightingSituation {
  if (session.state === 'faulted')
    return { kind: 'faulted', message: session.message };
  if (target === 'unknown' || session.state === 'unknown')
    return {
      kind: 'unknown',
      failure: sessionFailure(session),
      message: session.state === 'unknown' ? (session.message ?? null) : null,
    };
  if (target === 'off')
    return {
      kind: 'off',
      failure: session.state === 'stopped' ? (session.failure ?? null) : null,
    };
  if (target === 'screen_sync' && session.state === 'screen_sync')
    return { kind: 'sync', input };
  if (target === 'effects' && session.state === 'effects')
    return { kind: 'effects', failure: session.failure ?? null };
  const failure = sessionFailure(session);
  if (failure) {
    const at = failureSide(failure);
    const fromEffects = session.state === 'effects';
    return {
      kind: 'failed',
      target,
      failure,
      at,
      fromEffects,
      effectsOn: fromEffects && at !== 'output',
    };
  }
  return { kind: 'starting', target };
}

/** The failure a situation says, whether the lights are on or off. */
export function situationFailure(
  situation: LightingSituation,
): LightingFailure | null {
  switch (situation.kind) {
    case 'failed':
    case 'off':
    case 'effects':
    case 'unknown':
      return situation.failure;
    case 'faulted':
    case 'sync':
    case 'starting':
      return null;
    default:
      return unreachable(situation);
  }
}

/**
 * A situation in words: `headline` for a heading (no full stop), `sentence`
 * to read out. `effects` names the output's own effects (the target
 * `effects`).
 */
export function describeSituation(
  situation: LightingSituation,
  effects: string,
): { headline: string; sentence: string } {
  const both = (words: string) => ({ headline: words, sentence: `${words}.` });
  switch (situation.kind) {
    case 'faulted':
      return {
        headline: 'Prysm stopped responding',
        sentence: 'Prysm stopped responding. Restart Prysm to recover.',
      };
    case 'unknown':
      return {
        headline: 'Prysm is newer than this page',
        sentence:
          'Prysm is newer than this page. Reload it to see what the lights do.',
      };
    case 'off':
      return both('Lights are off');
    case 'failed':
      if (situation.target === 'effects')
        return both(`${effects} could not start`);
      return both(
        situation.fromEffects
          ? 'Screen sync could not start'
          : 'Screen sync stopped',
      );
    case 'sync':
      return {
        headline: 'Lights follow your screen',
        sentence: 'Screen sync is running.',
      };
    case 'effects':
      return both(`${effects} are on`);
    case 'starting': {
      const words =
        situation.target === 'screen_sync'
          ? 'Starting screen sync…'
          : `Starting ${effects}…`;
      return { headline: words, sentence: words };
    }
    default:
      return unreachable(situation);
  }
}

/**
 * Saved settings that do not validate, with the lights off: they name a
 * section's field, yet no device was asked, so that is all there is to say.
 */
export function unusableSettings(
  failure: Failure | LightingFailure,
  target: TargetReading,
) {
  return failure.kind === 'invalid' && target === 'off';
}

/**
 * A failure in plain words, from its typed parts only: `section` (the side
 * of the path it came from), `field` (the setting it concerns) and `kind`.
 * Without a side it is said by what stopped; the message is under Details.
 */
export function plainFailure(
  failure: Failure | LightingFailure,
  target: TargetReading,
  effects: string,
) {
  if (unusableSettings(failure, target))
    return 'Prysm can’t use these settings.';
  const section = failureSide(failure);
  if (section === 'capture') return 'The video source is not working.';
  if (section === 'output') return 'The lights are not responding.';
  // What a target newer than this page tried is not known.
  if (target === 'unknown') return 'Something went wrong.';
  switch (failure.kind) {
    case 'unavailable':
      return 'A device is not available right now.';
    case 'busy':
      // With the lights off there is nothing to try again.
      return target === 'off'
        ? 'Prysm is busy.'
        : 'Prysm is busy. Try again in a moment.';
    case 'adapter':
    case 'invalid':
      if (target === 'off') return 'The lights could not be switched off.';
      return target === 'effects'
        ? `${effects} could not start.`
        : 'Screen sync could not start.';
    default:
      return 'Something went wrong.';
  }
}

/** An adapter settings section: `{ kind, ...fields }`, typed by the host. */
export interface AdapterSettings {
  kind: string;
}

/** Generated settings with the host's capture and output section types. */
export type Settings<C, O> = Omit<GeneratedSettings, 'capture' | 'output'> & {
  capture?: C;
  output?: O;
};
/**
 * The runtime state the server sends whole on every change, as this page
 * reads it: a target or session state newer than this page is `unknown`.
 */
export type ApplicationSnapshot<C, O> = Omit<
  State,
  'settings' | 'target' | 'session'
> & {
  settings: Settings<C, O>;
  target: TargetReading;
  session: SessionReading;
};

/** A live state stream: the current state, then each later state. */
export interface Observation<C, O> {
  initial: ApplicationSnapshot<C, O>;
  updates: AsyncIterable<ApplicationSnapshot<C, O>>;
  close(): void;
}

/**
 * Commands resolve once the runtime has finished them; failures reject with an
 * {@link ApplicationError}. Closing an observation never commands lighting.
 */
export interface ApplicationClient<C, O> {
  observe(signal: AbortSignal): Promise<Observation<C, O>>;
  setTarget(target: LightingTarget): Promise<void>;
  /**
   * Show a calibration pattern, or end calibration with `null`. Needs the
   * target `screen_sync`; the engine ends it after ten minutes without a call.
   */
  setCalibration(calibration: CalibrationPattern | null): Promise<void>;
  /** Returns the new settings revision. */
  replaceSettings(expected: number, settings: Settings<C, O>): Promise<number>;
  /**
   * Light with `processing` without saving it, while a control is dragged.
   * A settings save ends it, and so do 30 seconds without another call.
   */
  tryProcessing(processing: Settings<C, O>['processing']): Promise<void>;
}

/** An open preview stream: events until closed or the portal goes away. */
export interface PreviewStream {
  events: AsyncIterable<PreviewEvent>;
  close(): void;
}

/**
 * The live preview. Each open stream counts as a viewer, and the session
 * encodes previews only while one is open, so keep it open only while shown.
 * Rejects with `busy` when the portal has too many viewers.
 */
export interface PreviewClient {
  preview(signal: AbortSignal): Promise<PreviewStream>;
}

/** An open colors stream: events until closed or the portal goes away. */
export interface ColorsStream {
  events: AsyncIterable<ColorsEvent>;
  close(): void;
}

/**
 * The colours the lights show, a few per edge, without the picture: light
 * enough for a page to tint with while it is shown. `colors` events come at
 * most ten times a second and only when they change; `idle` means the lights
 * are not showing Prysm's colours (off, the device's own effects, no picture).
 * Each open stream counts as a listener, and the session copies colours only
 * while one is open, so close it when it is not shown. Rejects with `busy`
 * when the portal has too many listeners.
 */
export interface ColorsClient {
  colors(signal: AbortSignal): Promise<ColorsStream>;
}

/**
 * What an adapter kind finds to choose from: the video devices present, the
 * lights answering on the network. An on-request lookup of up to about two
 * seconds, not state; nothing found is an empty list.
 */
export interface DiscoveryClient {
  candidates(
    section: FailureSection,
    kind: string,
    signal?: AbortSignal,
  ): Promise<Candidate[]>;
}

/**
 * What the capture a section selects delivers (`status: 'known'` with its
 * modes), or `unknown` when its hardware cannot be asked. An on-request
 * lookup of the section as it would be saved, not state; its size and rate
 * need not be valid.
 */
export interface CaptureModesClient {
  modes(capture: CaptureSettings, signal?: AbortSignal): Promise<Modes>;
}

export class ApplicationError extends Error {
  constructor(readonly failure: Failure) {
    super(failure.message);
  }
}

/** The server's failure behind `error`, or an `internal` one for anything else. */
export function failureOf(error: unknown): Failure {
  return error instanceof ApplicationError
    ? error.failure
    : {
        kind: 'internal',
        message: error instanceof Error ? error.message : String(error),
      };
}
