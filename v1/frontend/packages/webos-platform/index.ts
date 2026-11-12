/**
 * Television platform port, implemented by the Luna bridge. Its Rust
 * counterpart is not ported to the runtime yet, so these shapes are local.
 */
export interface TvHostStatus {
  service_ready: boolean;
  capture_available: boolean;
  forwarding: boolean;
}
export type TvHostCommand = 'inspect_service' | 'repair_service';

export interface TvHostClient {
  status(): Promise<TvHostStatus>;
  command(command: TvHostCommand): Promise<void>;
}

export function unavailableTvHost(): TvHostClient {
  const unavailable = (): Promise<never> =>
    Promise.reject(new Error('webOS platform bridge is not implemented.'));
  return { status: unavailable, command: unavailable };
}
