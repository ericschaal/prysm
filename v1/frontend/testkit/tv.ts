/* eslint-disable @typescript-eslint/require-await -- Fixtures implement async ports without I/O; thrown errors must reject promises. */
import type { TvHostClient, PatternPresenter } from '@prysm/webos-platform';

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

/** Acknowledges simulated playback only. No test pattern video is rendered. */
export function fakePatternPresenter() {
  const calls: string[] = [];
  const presenter: PatternPresenter = {
    present: async (pattern) => {
      calls.push(`present:${pattern.id}`);
      return { kind: 'ready' };
    },
    release: () => {
      calls.push('release');
    },
  };
  return { presenter, calls };
}
