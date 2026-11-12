/* eslint-disable @typescript-eslint/require-await -- Fixtures implement async ports without I/O; thrown errors must reject promises. */
import type { TvHostClient } from '@prysm/webos-platform';

export function fakeTvHost(forwarding: boolean) {
  const calls: string[] = [];
  const client: TvHostClient = {
    status: async () => ({
      service_ready: true,
      capture_available: true,
      forwarding,
    }),
    command: async (command) => {
      calls.push(command);
    },
  };
  return { client, calls };
}
