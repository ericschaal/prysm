import { describeSavedPicture, pictureFacts } from '@prysm/web-portal';

/** The picture settings a TV capture section has; omitted ones use defaults. */
export interface TvPicture {
  width?: number;
  height?: number;
  fps?: number;
  include_ui?: boolean;
}

/** A TV picture as Home's facts: size, rate and whether menus are captured. */
export function tvPictureFacts(
  value: TvPicture,
  defaults: Required<TvPicture>,
) {
  return [
    ...pictureFacts(value, defaults),
    {
      label: 'Menus',
      value:
        (value.include_ui ?? defaults.include_ui) ? 'Included' : 'Left out',
    },
  ];
}

/** `640 × 360 at 30 fps, with menus` for a TV picture. */
export function describeTvPicture(
  value: TvPicture,
  defaults: Required<TvPicture>,
) {
  const picture = describeSavedPicture(value, defaults);
  return `${picture}, ${(value.include_ui ?? defaults.include_ui) ? 'with' : 'without'} menus`;
}

export const includeMenusHelp =
  'Menus are captured over the video, and when nothing is playing (such as the Home screen) the TV’s own screens are captured. Off: video only.';

/** What Rust uses for an omitted field. */
export const webosDefaults = {
  width: 640,
  height: 360,
  fps: 30,
  include_ui: true,
} as const;
