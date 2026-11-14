import type {
  Calibration,
  Candidate,
  ColorsEvent,
  Failure,
  LightingTarget,
  OutputIssue,
  Pattern,
  Phase,
  PreviewEvent,
  Session,
  Settings as GeneratedSettings,
  State,
} from '@prysm/contracts/application';

export type {
  Calibration,
  Candidate,
  ColorsEvent,
  Failure,
  LightingTarget,
  OutputIssue,
  Pattern,
  PreviewEvent,
  Session,
};
export { same } from './same';

/**
 * Why the lights are not showing screen sync, for people. Prysm keeps
 * sending and does not switch the lights back; they follow again as soon as
 * they take its colours.
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
};

/**
 * What the lights are doing, in words, from the requested target and the
 * session. `effects` names the output's own effects (`device_native`).
 */
export function describeLighting({
  target,
  session,
  failure,
  effects,
}: {
  target: LightingTarget;
  session: Session;
  failure: Failure | null;
  effects: string;
}): string {
  switch (session.state) {
    case 'faulted':
      return `Prysm stopped responding while ${phases[session.phase]}. Restart Prysm to recover.`;
    case 'running':
      return 'Screen sync is running.';
    case 'native':
      return `${effects} are on.`;
    case 'stopped':
      if (target === 'off') return 'Lights are off.';
      if (failure)
        return target === 'processed'
          ? 'Screen sync stopped.'
          : `${effects} could not start.`;
      return target === 'processed'
        ? 'Starting screen sync…'
        : `Starting ${effects}…`;
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
  setCalibration(calibration: Calibration | null): Promise<void>;
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
