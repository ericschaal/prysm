import type { FileSettings } from '@prysm/contracts/host-desktop';
import {
  describePicture,
  pictureSize,
  type AdapterUi,
} from '@prysm/web-portal';
import { FileEditor } from './editor';
import { fileDefaults } from './settings';

export { fileDefaults, fileLimits } from './settings';

export const fileCapture: AdapterUi<FileSettings> = {
  // For development: it needs FFmpeg, which the packaged builds leave out.
  label: 'Video file (development only)',
  developer: true,
  create: () => ({ kind: 'file', path: '' }),
  Editor: FileEditor,
  normalize: (value) => ({ ...value, path: value.path.trim() }),
  summary: ({ path, width, height, fps }) =>
    `${path}, ${describePicture(width ?? fileDefaults.width, height ?? fileDefaults.height, fps ?? fileDefaults.fps)}`,
  transport: () => 'file',
  facts: ({ width, height, fps }) => [
    {
      label: 'Picture',
      value: pictureSize(
        width ?? fileDefaults.width,
        height ?? fileDefaults.height,
      ),
    },
    { label: 'Rate', value: `${fps ?? fileDefaults.fps} fps` },
  ],
};
