import type {
  Failure,
  LightingTarget,
  OutputIssue,
  PreviewEvent,
  Settings as GeneratedSettings,
  State,
} from '@prysm/contracts/application';

export type { Failure, LightingTarget, OutputIssue, PreviewEvent };

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
export type { PreviewFrame } from '@prysm/contracts/application';
export type { Session } from '@prysm/contracts/application';

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
  /** Returns the new settings revision. */
  replaceSettings(expected: number, settings: Settings<C, O>): Promise<number>;
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

export class ApplicationError extends Error {
  constructor(readonly failure: Failure) {
    super(failure.message);
  }
}
