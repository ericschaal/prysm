import type {
  CalibrationPattern,
  Candidate,
  CaptureSettings,
  ColorsEvent,
  Failure,
  FailureSection,
  Input,
  LightingTarget,
  Mode,
  Modes,
  OutputIssue,
  Pattern,
  Phase,
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

/** What a faulted session was doing when it stopped. */
const phases: Record<Phase, string> = {
  running: 'running',
  closing_source: 'closing the capture source',
  idling_output: 'idling the lights',
  finishing: 'finishing',
  source_unreleased: 'confirming the capture source was released',
};

/** What the lights show while on: screen sync or the output's own effects. */
export type LightingMode = Exclude<LightingTarget, 'off'>;

/**
 * What the lights are doing, decided once from the requested target, the
 * session and the failure; every view of the lights shows this.
 *
 * - `faulted`: the session stopped responding; nothing works until a restart.
 * - `off`: the lights were asked off (the session may still be stopping).
 * - `failed`: the mode asked for did not start; `at` is the side that
 *   failed, when known. After a failed start from effects the session stays
 *   `native`: the failure still wins.
 * - `sync`: screen sync runs; `input` says whether frames arrive, absent
 *   until measured.
 * - `effects`: the output's own effects run.
 * - `starting`: the mode asked for is not running yet (also while switching
 *   from the other one).
 */
export type LightingSituation =
  | { kind: 'faulted'; phase: Phase }
  | { kind: 'off' }
  | {
      kind: 'failed';
      target: LightingMode;
      failure: Failure;
      at: FailureSection | null;
    }
  | { kind: 'sync'; input: Input | undefined }
  | { kind: 'effects' }
  | { kind: 'starting'; target: LightingMode };

export function lightingSituation({
  target,
  session,
  failure,
  input,
}: {
  target: LightingTarget;
  session: Session;
  failure?: Failure | null;
  input?: Input;
}): LightingSituation {
  if (session.state === 'faulted')
    return { kind: 'faulted', phase: session.phase };
  if (target === 'off') return { kind: 'off' };
  if (failure) {
    // Which side failed: typed for adapter failures, else the setting named.
    const field = failure.field?.split('.')[0];
    const named = field === 'capture' || field === 'output' ? field : null;
    return { kind: 'failed', target, failure, at: failure.section ?? named };
  }
  if (target === 'processed' && session.state === 'running')
    return { kind: 'sync', input };
  if (target === 'device_native' && session.state === 'native')
    return { kind: 'effects' };
  return { kind: 'starting', target };
}

/**
 * A situation in words: `headline` for a heading (no full stop), `sentence`
 * to read out. `effects` names the output's own effects (`device_native`).
 */
export function describeSituation(
  situation: LightingSituation,
  effects: string,
): { headline: string; sentence: string } {
  const both = (words: string) => ({ headline: words, sentence: `${words}.` });
  switch (situation.kind) {
    case 'faulted': {
      // A phase newer than this page is not named.
      const doing = phases[situation.phase] as string | undefined;
      const words = `Prysm stopped responding${doing ? ` while ${doing}` : ''}. Restart Prysm to recover.`;
      return { headline: words, sentence: words };
    }
    case 'off':
      return both('Lights are off');
    case 'failed':
      return both(
        situation.target === 'processed'
          ? 'Screen sync stopped'
          : `${effects} could not start`,
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
        situation.target === 'processed'
          ? 'Starting screen sync…'
          : `Starting ${effects}…`;
      return { headline: words, sentence: words };
    }
  }
}

/**
 * A failure in plain words, from its typed parts only: `section` (the side
 * of the path it came from), `field` (the setting it concerns) and `kind`.
 * Without a side it is said by what stopped; the message is under Details.
 */
export function plainFailure(
  failure: Failure,
  target: LightingTarget,
  effects: string,
) {
  const section = failure.section ?? failure.field?.split('.')[0];
  if (section === 'capture') return 'The video source is not working.';
  if (section === 'output') return 'The lights are not responding.';
  switch (failure.kind) {
    case 'timeout':
      return 'A device took too long to answer.';
    case 'unavailable':
      return 'A device is not available right now.';
    case 'busy':
      return 'Prysm is busy. Try again in a moment.';
    case 'adapter':
    case 'invalid':
      return target === 'device_native'
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
/** The runtime state the server sends whole on every change. */
export type ApplicationSnapshot<C, O> = Omit<State, 'settings'> & {
  settings: Settings<C, O>;
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
   * target `processed`; the engine ends it after ten minutes without a call.
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
    section: 'capture' | 'output',
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
