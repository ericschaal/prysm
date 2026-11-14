import { generatePath } from 'react-router';

/**
 * Every page's address under the hash (`#/settings`), by the page's id. The
 * TV's QR code links to `calibrate`; anything else not here shows Home.
 */
export const pages = {
  home: '/',
  settings: '/settings',
  video: '/settings/video',
  lights: '/settings/lights',
  calibrations: '/settings/calibrations',
  calibration: '/settings/calibrations/:id',
  picture: '/settings/picture',
  appearance: '/settings/appearance',
  about: '/settings/about',
  preview: '/preview',
  calibrate: '/calibrate',
} as const;

export type PageId = keyof typeof pages;

/** A page to open: by its id, or one calibration's page by the calibration. */
export type Place = Exclude<PageId, 'calibration'> | { calibration: string };

/** The router path of `place`: `'settings'` → `/settings`. */
export const pathTo = (place: Place) =>
  typeof place === 'string'
    ? pages[place]
    : generatePath(pages.calibration, { id: place.calibration });
