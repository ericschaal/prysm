import type { Status } from './service';

export const embeddedBuild = import.meta.env.VITE_EMBEDDED_PRYSM === '1';

export function supportsEmbedded(status: Status | null) {
  return embeddedBuild && status?.capabilities.embeddedPortal === true;
}
