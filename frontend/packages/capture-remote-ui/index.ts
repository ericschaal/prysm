import { captureDefaults } from '@prysm/contracts/constants';
import type { RemoteReceiverSettings } from '@prysm/contracts/host-desktop';
import { describeTvPicture, tvPictureFacts } from '@prysm/capture-webos-ui';
import type { AdapterUi } from '@prysm/web-portal';
import { RemoteEditor } from './editor';

export * from './settings';

const defaults = captureDefaults.remote;

export const remoteCapture: AdapterUi<RemoteReceiverSettings> = {
  label: 'LG TV on your network',
  create: () => ({ kind: 'remote', ...defaults }),
  optional: defaults,
  Editor: RemoteEditor,
  summary: (value) => `From the TV, ${describeTvPicture(value, defaults)}`,
  facts: (value) => tvPictureFacts(value, defaults),
  transport: (value) =>
    (value.encoding ?? defaults.encoding) === 'raw' ? 'raw' : 'h264',
  setup: 'Give the TV app the address of this page.',
  trouble: 'If the TV can’t send its picture, try another way under Advanced.',
  problems: {
    'capture.encoding':
      'This TV can’t send the picture this way. Choose another.',
  },
};
