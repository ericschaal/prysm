import type { FileSettings } from '@prysm/contracts/host-desktop';
import { describePicture, type AdapterUi } from '@prysm/web-portal';
import { FileEditor } from './editor';
import { fileDefaults } from './settings';

export { fileDefaults, fileLimits } from './settings';

export const fileCapture: AdapterUi<FileSettings> = {
  label: 'Video file',
  create: () => ({ kind: 'file', path: '' }),
  Editor: FileEditor,
  normalize: (value) => ({ ...value, path: value.path.trim() }),
  summary: ({ path, width, height, fps }) =>
    `${path}, ${describePicture(width ?? fileDefaults.width, height ?? fileDefaults.height, fps ?? fileDefaults.fps)}`,
};
