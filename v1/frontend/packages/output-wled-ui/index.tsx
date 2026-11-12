import type { WledSettings } from '@prysm/contracts/host-desktop';
import type { SettingsPanelProps } from '@prysm/web-portal';

/** Minimal panel: edits the address and keeps the LED layout. */
export function WledOutputSettings({
  settings,
  disabled,
  onSubmit,
}: SettingsPanelProps<WledSettings>) {
  return (
    <form
      key={JSON.stringify(settings)}
      onSubmit={(event) => {
        event.preventDefault();
        const address = new FormData(event.currentTarget).get('address');
        if (typeof address !== 'string') return;
        void onSubmit({
          kind: 'wled',
          address,
          leds: settings?.leds ?? {},
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
