import type { NullSettings } from '@prysm/contracts/host-desktop';
import type { AdapterUi } from '@prysm/web-portal';

/** Output kind `null`: runs screen sync without driving any LEDs. */
export const nullOutput: AdapterUi<NullSettings> = {
  label: 'No lights (test)',
  // For checking capture and processing: offered with developer options.
  developer: true,
  create: () => ({ kind: 'null' }),
  Editor: () => (
    <p className="help">
      Screen sync runs without driving any LEDs, for checking capture and
      processing. There is nothing to configure.
    </p>
  ),
  // No device: no effects of its own and nothing to open.
  lighting: () => ({}),
};
