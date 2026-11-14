/** The hash route of the Settings page. */
export const settingsRoute = '#/settings';
/** The hash route of the live preview, opened from Settings. */
export const previewRoute = '#/preview';
/** The hash routes of the video source and lights editors, under Settings. */
export const videoRoute = '#/settings/video';
export const lightsRoute = '#/settings/lights';
/** The hash routes of the colour profiles and picture fine-tuning pages. */
export const profilesRoute = '#/settings/profiles';
export const pictureRoute = '#/settings/picture';
/** The hash routes of the appearance and about pages. */
export const appearanceRoute = '#/settings/appearance';
export const aboutRoute = '#/settings/about';
/** The hash route of the Calibrate page; the TV shows a QR code of it. */
export const calibrateRoute = '#/calibrate';

/** A `#/…` route as a router path: `#/settings` → `/settings`, `''` → `/`. */
export const pathOfRoute = (route: string) => route.replace(/^#/, '') || '/';
