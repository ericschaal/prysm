import type { ColorsClient, ColorsEvent } from '@prysm/application-client';
import { fakeEventStreams } from './streams';

/**
 * In-memory colors streams: counts opens and closes, delivers scripted
 * events to every open stream, and can refuse the next open.
 */
export function fakeColorsClient() {
  const { connect, controls } = fakeEventStreams<ColorsEvent>();
  const client: ColorsClient = { colors: connect };
  return { client, ...controls };
}
