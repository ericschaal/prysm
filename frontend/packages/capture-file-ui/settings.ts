import type { PicturePreset } from '@prysm/web-portal';

/**
 * Limits from the generated `FileSettings` schema. `fps` has no schema
 * maximum; the Rust `u32` bounds it.
 */
export const fileLimits = {
  width: { min: 1, max: 3840, step: 1 },
  height: { min: 1, max: 2160, step: 1 },
  fps: { min: 1, max: 4294967295, step: 1 },
} as const;
/** What Rust uses for an omitted field. */
export const fileDefaults = { width: 640, height: 360, fps: 30 } as const;
/**
 * A file plays at its own rate when that is slower, and Prysm cannot know
 * the rate before it plays, so Smooth is not offered: 60 fps would promise
 * what most files do not have. Custom is there for one that does.
 */
export const filePresets: readonly PicturePreset[] = ['balanced', 'efficient'];
