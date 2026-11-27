import type { Picture } from './adapters';

/**
 * Each preset in words and the size and rate it aims for; what a source
 * delivers decides what it sets (`offeredPresets`). The label is what the
 * user gains, the line beneath what it costs.
 */
export const picturePresets = {
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
} as const satisfies Record<
  string,
  Required<Picture> & { label: string; about: string }
>;

export type PicturePreset = keyof typeof picturePresets;
/** Targets in display order, used until capture reports its modes. */
export const presetTargets = (
  Object.keys(picturePresets) as PicturePreset[]
).map((preset) => ({ preset, ...picturePresets[preset] }));

const rates = [...new Set(presetTargets.map(({ fps }) => fps))].sort(
  (a, b) => a - b,
);

/** The rates the presets aim for, in words: `30 or 60 fps`. */
export const presetRates = `${rates.join(' or ')}\u00a0fps`;

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
