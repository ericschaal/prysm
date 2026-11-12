import type { DeviceSettings } from '@prysm/contracts/host-desktop';
import type { SettingsPanelProps } from '@prysm/web-portal';

/** Minimal panel: edits the device path and keeps the other fields. */
export function DeviceCaptureSettings({
  settings,
  disabled,
  onSubmit,
}: SettingsPanelProps<DeviceSettings>) {
  return (
    <form
      key={JSON.stringify(settings)}
      onSubmit={(event) => {
        event.preventDefault();
        const path = new FormData(event.currentTarget).get('path');
        if (typeof path !== 'string') return;
        void onSubmit({ ...settings, kind: 'device', path });
      }}
    >
      <h2>Video device capture</h2>
      <fieldset disabled={disabled}>
        <legend>Device</legend>
        <label>
          Device path{' '}
          <input name="path" required defaultValue={settings?.path ?? ''} />
        </label>
        <button type="submit">Save capture settings</button>
      </fieldset>
    </form>
  );
}
