import type { RemoteReceiverSettings } from '@prysm/contracts/host-desktop';
import { describeTvPicture, tvPictureFacts } from '@prysm/capture-webos-ui';
import type { AdapterUi } from '@prysm/web-portal';
import { RemoteEditor } from './editor';
import { remoteDefaults } from './settings';

export * from './settings';

export const remoteCapture: AdapterUi<RemoteReceiverSettings> = {
  label: 'LG TV on your network',
  create: () => ({ kind: 'remote' }),
  Editor: RemoteEditor,
  summary: (value) =>
    `From the TV, ${describeTvPicture(value, remoteDefaults)}`,
  facts: (value) => tvPictureFacts(value, remoteDefaults),
  // How the picture travels: H.264 (either encoder) or raw frames.
  transport: (value) =>
    (value.encoding ?? remoteDefaults.encoding) === 'raw' ? 'raw' : 'h264',
  setup: 'Give the TV app the address of this page.',
  trouble: 'If the TV can’t send its picture, try another way under Advanced.',
  problems: {
    'capture.encoding':
      'This TV can’t send the picture this way. Choose another.',
  },
};
