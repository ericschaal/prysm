import type { Picture } from './adapters';

/** The capture sizes and rates offered by name; anything else is Custom. */
export type PicturePreset = 'balanced' | 'smooth' | 'efficient';

/**
 * Each preset in words and in numbers, in the order offered. The label is
 * what the user gains, the line beneath what it costs.
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

/** The presets a source that can capture 60 fps offers. */
export const allPicturePresets: readonly PicturePreset[] = [
  'balanced',
  'smooth',
  'efficient',
];

/** The row for any other size and rate: it opens the number fields. */
export const customPicture = {
  label: 'Custom',
  about: 'Your own size and rate.',
} as const;

/** The paths of the three numbers a preset sets. */
export const picturePaths = ['capture.width', 'capture.height', 'capture.fps'];

/** The offered preset `value` is at (omitted fields at `defaults`), if any. */
export function presetOf(
  value: Picture,
  defaults: Required<Picture>,
  offered: readonly PicturePreset[],
): PicturePreset | null {
  const width = value.width ?? defaults.width;
  const height = value.height ?? defaults.height;
  const fps = value.fps ?? defaults.fps;
  return (
    offered.find((preset) => {
      const at = picturePresets[preset];
      return at.width === width && at.height === height && at.fps === fps;
    }) ?? null
  );
}
