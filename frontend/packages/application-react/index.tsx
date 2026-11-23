import { useCallback, useSyncExternalStore } from 'react';
import type { ApplicationRuntime, Connection } from '@prysm/application-state';

/**
 * The connection of `application` (its status and latest state), rendered
 * again on every change; `null` without one. It holds no observation lease:
 * whoever shows the application calls `application.connect()`.
 */
export function useConnection<C, O>(
  application: ApplicationRuntime<C, O>,
): Connection<C, O>;
export function useConnection<C, O>(
  application: ApplicationRuntime<C, O> | null,
): Connection<C, O> | null;
export function useConnection<C, O>(
  application: ApplicationRuntime<C, O> | null,
): Connection<C, O> | null {
  const subscribe = useCallback(
    (changed: () => void) =>
      application ? application.store.subscribe(changed) : () => {},
    [application],
  );
  return useSyncExternalStore(subscribe, () =>
    application
      ? application.selectors.selectConnection(application.store.getState())
      : null,
  );
}
