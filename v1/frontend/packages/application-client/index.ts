import type {
  Accepted,
  ClientFailure,
  ClientSnapshot_Value_Value,
  ClientUpdate_Value_Value,
  ConfigureRequest_Value,
  OperationId,
  OperationStatus,
  TargetRequest,
  CalibrationPresentation,
  PresentationFeedback,
  EndPresentationRequest,
  PatternRequest,
  PlaybackFeedback,
} from '@prysm/contracts/application';

export type {
  Accepted,
  ClientFailure,
  TargetRequest,
  OperationId,
  OperationStatus,
  CalibrationPresentation,
  PatternRequest,
  PlaybackFeedback,
};
// OpenAPI specializes generics; restore only the two generated settings slots for UI reuse.
export type ApplicationSnapshot<C, O> = Omit<
  ClientSnapshot_Value_Value,
  'capture' | 'output'
> & { capture: C | null; output: O | null };
export type ConfigureRequest<S> = Omit<ConfigureRequest_Value, 'settings'> & {
  settings: S;
};
export type ApplicationUpdate<C, O> =
  | (Omit<
      Extract<ClientUpdate_Value_Value, { kind: 'snapshot' }>,
      'snapshot'
    > & { snapshot: ApplicationSnapshot<C, O> })
  | Extract<ClientUpdate_Value_Value, { kind: 'resync_required' }>;

export interface Observation<C, O> {
  initial: ApplicationSnapshot<C, O>;
  updates: AsyncIterable<ApplicationUpdate<C, O>>;
  close(): void;
}

export interface CalibrationClient {
  open(id: OperationId): Promise<CalibrationPresentation>;
  feedback(request: PresentationFeedback): Promise<void>;
  close(request: EndPresentationRequest): Promise<void>;
}

/** observe atomically establishes a snapshot and its subscription. Closing observes only. */
export interface ApplicationClient<C, O> {
  observe(signal: AbortSignal): Promise<Observation<C, O>>;
  configureCapture(request: ConfigureRequest<C>): Promise<Accepted>;
  configureOutput(request: ConfigureRequest<O>): Promise<Accepted>;
  setTarget(request: TargetRequest): Promise<Accepted>;
  operation(id: OperationId): Promise<OperationStatus>;
  calibration: CalibrationClient;
}

export class ApplicationError extends Error {
  constructor(readonly detail: ClientFailure) {
    let message: string;
    switch (detail.kind) {
      case 'outcome_unknown':
        message = `Command ${detail.operation.instance}/${detail.operation.request} has an unknown outcome; check this operation before retrying.`;
        break;
      case 'unknown_operation':
        message = 'Operation is unknown or expired.';
        break;
      default:
        message = detail.message;
    }
    super(message);
  }
}

export function newRequestId(instance: string): OperationId {
  const words = crypto.getRandomValues(new Uint32Array(2));
  const request = (BigInt(words[0]!) << 32n) | BigInt(words[1]!);
  return { instance, request: (request || 1n).toString() };
}
