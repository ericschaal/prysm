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
  profiles: '/settings/profiles',
  profile: '/settings/profiles/:id',
  picture: '/settings/picture',
  appearance: '/settings/appearance',
  about: '/settings/about',
  preview: '/preview',
  calibrate: '/calibrate',
} as const;

export type PageId = keyof typeof pages;

/** A page to open: by its id, or one colour profile's page by the profile. */
export type Place = Exclude<PageId, 'profile'> | { profile: string };

/** The router path of `place`: `'settings'` → `/settings`. */
export const pathTo = (place: Place) =>
  typeof place === 'string'
    ? pages[place]
    : generatePath(pages.profile, { id: place.profile });
