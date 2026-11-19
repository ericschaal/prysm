import type { Picture } from './adapters';

/** The capture sizes and rates offered by name. */
export type PicturePreset = 'balanced' | 'smooth' | 'efficient';

/**
 * Each preset in words and the size and rate it aims for; what a source
 * delivers decides what it sets (`offeredPresets`). The label is what the
 * user gains, the line beneath what it costs.
 */
export const picturePresets: Readonly<
  Record<PicturePreset, Required<Picture> & { label: string; about: string }>
> = {
  balanced: {
    label: 'Balanced',
    about: 'Right for most setups.',
    width: 640,
    height: 360,
    fps: 30,
  },
  smooth: {
    label: 'Smooth',
    about: 'The lights update twice as often. More work for the processor.',
    width: 640,
    height: 360,
    fps: 60,
  },
  efficient: {
    label: 'Efficient',
    about: 'Less work for the processor. Slightly softer colours.',
    width: 320,
    height: 180,
    fps: 30,
  },
};

/** The presets in the order offered; the first is the one to fall back to. */
export const presetOrder: readonly PicturePreset[] = [
  'balanced',
  'smooth',
  'efficient',
];

/** The row for a saved size and rate no preset sets: shown, not chosen. */
export const customPicture = {
  label: 'Custom',
  about: 'Kept until you choose another.',
} as const;

/** The paths of the three numbers a preset sets. */
export const picturePaths = [
  'capture.width',
  'capture.height',
  'capture.fps',
] as const;
