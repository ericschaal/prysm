import type { WebosSettings } from '@prysm/contracts/host-webos';
import type { AdapterUi } from '@prysm/web-portal';
import { WebosEditor } from './editor';
import { describeTvPicture, webosDefaults } from './settings';

export { TvPictureFields } from './editor';
export * from './settings';

export const webosCapture: AdapterUi<WebosSettings> = {
  label: 'This TV',
  create: () => ({ kind: 'webos' }),
  Editor: WebosEditor,
  summary: (value) => describeTvPicture(value, webosDefaults),
};
