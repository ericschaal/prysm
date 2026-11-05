import type { SettingsPanelProps } from '@prysm/web-portal';

import type { WledSettings as WledOutputConfig } from '@prysm/contracts/wled';

/** Minimal composition panel; LED layout controls remain deferred. */
export function WledOutputSettings({
  settings,
  disabled,
  onSubmit,
}: SettingsPanelProps<WledOutputConfig>) {
  return (
    <form
      key={JSON.stringify(settings)}
      onSubmit={(event) => {
        event.preventDefault();
        const data = new FormData(event.currentTarget);
        const address = data.get('address');
        if (typeof address !== 'string') return;
        void onSubmit({
          address,
          leds: settings?.leds ?? [0, 0, 0, 0],
        });
      }}
    >
      <h2>WLED output</h2>
      <fieldset disabled={disabled}>
        <legend>Device</legend>
        <label>
          Address{' '}
          <input
            name="address"
            required
            defaultValue={settings?.address ?? ''}
          />
        </label>
        <button type="submit">Save output settings</button>
      </fieldset>
    </form>
  );
}
