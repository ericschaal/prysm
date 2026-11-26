/** Where the lighting modes go. */
import type { TargetReading } from '@prysm/webos-platform';
import type { ServiceLink } from './status';

/**
 * Where a lighting mode is sent: to the portal of the Prysm that drives the
 * lights, or, in the full package while the TV service asks (no worker
 * runs, so no portal answers), to the service, whose `setTarget` answers
 * what it asks and starts the worker again with that target (`target`, the
 * one it keeps meanwhile).
 */
export type ModeRoute =
  { via: 'portal' } | { via: 'service'; target: TargetReading };

/** Where a lighting mode is sent, by the service subscription. */
export function modeRoute(link: ServiceLink): ModeRoute {
  if (link.status !== 'live' && link.status !== 'lost')
    return { via: 'portal' };
  const { state } = link;
  return state.flavor === 'full' && state.worker.phase === 'asking'
    ? { via: 'service', target: state.target }
    : { via: 'portal' };
}
