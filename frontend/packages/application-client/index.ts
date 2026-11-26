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
  Picture,
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
  Picture,
  PreviewEvent,
  Session,
};
export { same } from './same';
export { reconnectDelayMs, STALE_MS, streamStep } from './recovery';

/**
 * The two sides an adapter fills, capture and output: the generated failure
 * sides.
 */
export type AdapterSide = FailureSection;

/**
 * Keyed by every value of a generated union, so the compiler keeps them in
 * step; read with {@link known}.
 */
export const lightingTargets: Record<LightingTarget, true> = {
  off: true,
  screen_sync: true,
  effects: true,
};
export const failureSections: Record<FailureSection, true> = {
  capture: true,
  output: true,
};
export const pictures: Record<Picture, true> = {
  arriving: true,
  quiet: true,
  refused: true,
};

/**
 * Whether frames arrive, as `input` says it; a value newer than this page
 * is `quiet` (no frames, no reason given).
 */
export const pictureOf = (input: Input): Picture =>
  known(pictures, input.picture, 'quiet');

/** A decoded JSON object's fields, before each is checked. */
export type Fields = Partial<Record<string, unknown>>;

/** Whether `value` is a JSON object: not `null`, not an array. */
export const isObject = (value: unknown): value is Fields =>
  !!value && typeof value === 'object' && !Array.isArray(value);

/** `raw` if it is one of `values`, else `fallback`: a value newer than this page. */
export function known<T extends string, F extends string | null | undefined>(
  values: Record<T, true>,
  raw: string,
  fallback: F,
): T | F {
  return Object.keys(values).indexOf(raw) >= 0 ? (raw as T) : fallback;
}

/**
 * Why no picture arrives while the source's device refuses to share the
 * video it shows (`picture` is `refused`), for people.
 */
export const REFUSED_VIDEO = 'The TV doesn’t let Prysm capture this video.';

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
      return 'The lights aren’t taking Prysm’s colours. In WLED, check that Sync Interfaces receives UDP realtime (DDP).';
    case 'other_sender':
      return 'Another program is sending colours to the lights. Stop it to see screen sync alone.';
    case 'unreachable':
      return 'The lights don’t answer. Check that they are powered and on the network.';
    default:
      return 'The lights aren’t showing screen sync.';
  }
}

/**
 * The headline while the lights are not showing screen sync, and while they
 * don't answer whatever was chosen: lights that don't answer are a problem;
 * any other issue (a newer one too) is a warning.
 */
export function outputIssueHeadline(issue: OutputIssue): string {
  return issue === 'unreachable'
    ? 'Can’t reach the lights'
    : 'Not showing screen sync';
}
export type { PreviewFrame } from '@prysm/contracts/application';

/**
 * What happens once lights that don't answer answer again, by the target:
 * Prysm keeps it and tries again every few seconds. `effects` names the
 * output's own effects. At most 52 characters with `WLED effects`.
 */
export function whenLightsAnswer(
  target: LightingTarget,
  effects: string,
): string {
  switch (target) {
    case 'screen_sync':
      return 'The lights follow again as soon as they answer.';
    case 'off':
      return 'The lights switch off as soon as they answer.';
    case 'effects':
      return `${effects} start as soon as the lights answer.`;
    default:
      return unreachable(target);
  }
}

/** What the lights show while on: screen sync or the output's own effects. */
export type LightingMode = Exclude<LightingTarget, 'off'>;

/** A target reached without a session: the lights off or their own effects. */
export type IdleTarget = Exclude<LightingTarget, 'screen_sync'>;

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
  const side = (raw: string | undefined) =>
    raw === undefined ? null : known(failureSections, raw, null);
  return (
    side('section' in failure ? failure.section : undefined) ??
    side('field' in failure ? failure.field?.split('.')[0] : undefined)
  );
}

/**
 * What the lights are doing, decided once from the requested target and the
 * session (with the failure it carries); every view of the lights shows
 * this. The rule is ordered, so every state lands on exactly one:
 *
 * - `faulted`: the session stopped responding; nothing works until a
 *   restart. `message` says what did not finish.
 * - `unknown`: the target or the session state is newer than this page.
 * - `unanswered`: the lights don't answer the request that puts them off or
 *   in their own effects (no session). The target stays; Prysm tries again
 *   every few seconds and applies it once they answer. `failure`: the saved
 *   settings could not be read or do not validate. (Lights that don't
 *   answer screen sync are `sync`: its session waits for them.)
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
  | { kind: 'unanswered'; target: IdleTarget; failure: LightingFailure | null }
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
  // Screen sync asked during a wait: its start ends the wait.
  if (session.state === 'unanswered' && target !== 'screen_sync')
    return { kind: 'unanswered', target, failure: session.failure ?? null };
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
      // Settings Prysm can't use never reached the lights.
      effectsOn: fromEffects && (at !== 'output' || unusableSettings(failure)),
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
    case 'unanswered':
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
    case 'unanswered': {
      const headline = outputIssueHeadline('unreachable');
      return {
        headline,
        sentence: `${headline}. ${whenLightsAnswer(situation.target, effects)}`,
      };
    }
    case 'off':
      return both('Lights are off');
    case 'failed':
      if (situation.target === 'effects')
        return both(`${effects} couldn’t start`);
      return both(
        situation.fromEffects
          ? 'Screen sync couldn’t start'
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
 * Settings Prysm can't use, whatever the lights were asked to do: they do
 * not validate, or the video source says it can't deliver them. A setting is
 * the fix, so that is all there is to say, even when the field names a side.
 */
export function unusableSettings(failure: Failure | LightingFailure) {
  return failure.kind === 'invalid';
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
  if (unusableSettings(failure)) return 'Prysm can’t use these settings.';
  const section = failureSide(failure);
  if (section === 'capture') return 'The video source isn’t working.';
  if (section === 'output') return 'The lights aren’t responding.';
  // What a target newer than this page tried is not known.
  if (target === 'unknown') return 'Something went wrong.';
  switch (failure.kind) {
    case 'unavailable':
      return 'A device isn’t available right now.';
    case 'busy':
      // With the lights off there is nothing to try again.
      return target === 'off'
        ? 'Prysm is busy.'
        : 'Prysm is busy. Try again in a moment.';
    case 'adapter':
      if (target === 'off') return 'The lights couldn’t be switched off.';
      return target === 'effects'
        ? `${effects} couldn’t start.`
        : 'Screen sync couldn’t start.';
    default:
      return 'Something went wrong.';
  }
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
    section: AdapterSide,
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
