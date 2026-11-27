import type { NullSettings } from '@prysm/contracts/host-desktop';
import type { AdapterUi } from '@prysm/web-portal';

export const nullOutput: AdapterUi<NullSettings> = {
  label: 'No lights (test)',
  // For checking capture and processing: offered with developer options.
  developer: true,
  create: () => ({ kind: 'null' }),
  optional: {},
  Editor: () => (
    <p className="help">
      Screen sync runs without driving any LEDs, for checking capture and
      processing. There is nothing to configure.
    </p>
  ),
  lighting: () => ({}),
};
