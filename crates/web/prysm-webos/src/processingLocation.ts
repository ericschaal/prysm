import type { Config, Status } from './service';

export const embeddedBuild = import.meta.env.VITE_EMBEDDED_PRYSM === '1';

export function supportsEmbedded(status: Status | null) {
  return embeddedBuild && status?.capabilities.embeddedPrysm === true;
}

export const edges = ['top', 'right', 'bottom', 'left'] as const;

export function sameOutput(
  a: Config['embeddedOutput'],
  b: Config['embeddedOutput'],
) {
  if (!a || !b) return !a && !b;
  return (
    a.url === b.url && edges.every((edge) => a.leds[edge] === b.leds[edge])
  );
}
