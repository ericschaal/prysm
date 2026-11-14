/**
 * Defaults and limits from the generated `DeviceSettings` schema. `fps` has no
 * schema maximum; it is bounded here by AVFoundation's `i32` timescale, the
 * stricter of the two platforms (the Rust check on macOS).
 */
export const deviceLimits = {
  width: { min: 1, max: 3840, step: 1 },
  height: { min: 1, max: 2160, step: 1 },
  fps: { min: 1, max: 2147483647, step: 1 },
} as const;
/** What Rust uses for an omitted field. */
export const deviceDefaults = { width: 640, height: 360, fps: 30 } as const;
