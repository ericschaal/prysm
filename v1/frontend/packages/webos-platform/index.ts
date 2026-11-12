/**
 * Television platform port, to be implemented by a Luna client of the TV
 * supervisor. These shapes are provisional and local: the supervisor's
 * values are generated in `@prysm/contracts/luna` (`State`, `Preferences`),
 * which the Luna client will adopt together with the TV UI.
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
