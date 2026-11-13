import { describePicture } from '@prysm/web-portal';

/**
 * Limits of the picture a TV captures, the same for this TV (`WebosSettings`)
 * and for a capture-only TV (`RemoteReceiverSettings`); NV12 needs even sizes
 * (the schemas say so with `multipleOf`, so the step of 2 is written here).
 */
export const tvPictureLimits = {
  width: { min: 2, max: 1920, step: 2 },
  height: { min: 2, max: 1080, step: 2 },
  fps: { min: 1, max: 60, step: 1 },
} as const;

/** The picture settings a TV capture section has; omitted ones use defaults. */
export interface TvPicture {
  width?: number;
  height?: number;
  fps?: number;
  include_ui?: boolean;
}

/** `640 × 360 at 30 fps, with menus` for a TV picture. */
export function describeTvPicture(
  value: TvPicture,
  defaults: Required<TvPicture>,
) {
  const picture = describePicture(
    value.width ?? defaults.width,
    value.height ?? defaults.height,
    value.fps ?? defaults.fps,
  );
  return `${picture}, ${(value.include_ui ?? defaults.include_ui) ? 'with' : 'without'} menus`;
}

/** Above this many pixels the lights gain nothing visible. */
export const enoughPixels = 640 * 360;
const size = '640\u00a0×\u00a0360';
/** The quiet hint shown for a size above 640 × 360. */
export const largeSizeHint = `${size} is enough for the lights. Larger sizes use more of the TV’s processor without a visible difference.`;

export const includeMenusHelp =
  'Menus are captured over the video, and when nothing is playing (such as the Home screen) the TV’s own screens are captured. Off: video only.';

/** What Rust uses for an omitted field. */
export const webosDefaults = {
  width: 640,
  height: 360,
  fps: 30,
  include_ui: true,
} as const;
