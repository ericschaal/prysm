import type { NullSettings } from '@prysm/contracts/host-desktop';
import type { AdapterUi } from '@prysm/web-portal';

/** Output kind `null`: runs screen sync without driving any LEDs. */
export const nullOutput: AdapterUi<NullSettings> = {
  label: 'No lights (test)',
  create: () => ({ kind: 'null' }),
  Editor: () => (
    <p className="help">
      Screen sync runs without driving any LEDs, for checking capture and
      processing. There is nothing to configure.
    </p>
  ),
  facts: () => [{ label: 'LEDs', value: 'None' }],
  // No device: no effects of its own and nothing to open.
  lighting: () => ({}),
};
