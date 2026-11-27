import { captureDefaults } from '@prysm/contracts/constants';
import type { WebosSettings } from '@prysm/contracts/host-webos';
import type { AdapterUi } from '@prysm/web-portal';
import { WebosEditor } from './editor';
import { describeTvPicture, tvPictureFacts } from './settings';

export { IncludeTvMenus } from './editor';
export * from './settings';

const defaults = captureDefaults.webos;

export const webosCapture: AdapterUi<WebosSettings> = {
  label: 'This TV',
  create: () => ({ kind: 'webos', ...defaults }),
  optional: defaults,
  Editor: WebosEditor,
  summary: (value) => describeTvPicture(value, defaults),
  facts: (value) => tvPictureFacts(value, defaults),
  transport: () => 'built_in',
};
