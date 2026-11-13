import type { WledSettings } from '@prysm/contracts/host-desktop';
import type { AdapterUi } from '@prysm/web-portal';
import { normalizeWledAddress, wledUrl } from './address';
import { WledEditor } from './editor';
import { ledTotal } from './leds';

export { normalizeWledAddress, wledUrl } from './address';
export { LedLayout } from './layout';
export {
  ledEdgeLimits,
  ledRuns,
  ledTotal,
  ledTotalLimits,
  wiring,
} from './leds';

export const wledOutput: AdapterUi<WledSettings> = {
  label: 'WLED',
  create: () => ({ kind: 'wled', address: '', leds: {} }),
  Editor: WledEditor,
  normalize: (value) => ({
    ...value,
    address: normalizeWledAddress(value.address),
  }),
  summary: ({ address, leds }) => `${address}, ${ledTotal(leds)} LEDs`,
  facts: ({ address, leds }) => [
    { label: 'LEDs', value: String(ledTotal(leds)) },
    { label: 'Address', value: address },
  ],
  lighting: (saved) => {
    const href = wledUrl(saved.address);
    return {
      effects: 'WLED effects',
      ...(href ? { link: { label: 'Open WLED', href } } : {}),
    };
  },
  edges: ({ leds }) => ({
    top: leds.top ?? 0,
    right: leds.right ?? 0,
    bottom: leds.bottom ?? 0,
    left: leds.left ?? 0,
  }),
};
