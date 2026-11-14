import type { PreviewClient, PreviewEvent } from '@prysm/application-client';
import { fakeEventStreams } from './streams';

/**
 * In-memory preview streams: counts opens and closes, delivers scripted
 * events to every open stream, and can refuse the next open.
 */
export function fakePreviewClient() {
  const { connect, controls } = fakeEventStreams<PreviewEvent>();
  const client: PreviewClient = { preview: connect };
  return { client, ...controls };
}
