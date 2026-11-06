import type { SettingsPanelProps } from '@prysm/web-portal';

import type { DeviceCaptureSettings as DeviceCaptureConfig } from '@prysm/contracts/device';

/** Minimal composition panel; resolution/rate controls remain deferred. */
export function DeviceCaptureSettings({
  settings,
  disabled,
  onSubmit,
}: SettingsPanelProps<DeviceCaptureConfig>) {
  const defaultLocation = settings?.path ?? '';
  return (
    <form
      key={JSON.stringify(settings)}
      onSubmit={(event) => {
        event.preventDefault();
        const data = new FormData(event.currentTarget);
        const source = data.get('source');
        if (source !== 'device' && source !== 'file') return;
        const location = data.get('location');
        if (typeof location !== 'string') return;
        const resolution: DeviceCaptureConfig['resolution'] =
          settings?.resolution ?? [320, 180];
        const common = {
          resolution,
          rate: settings?.rate ?? 30,
        };
        void onSubmit({ ...common, source, path: location });
      }}
    >
      <h2>Video device capture</h2>
      <fieldset disabled={disabled}>
        <legend>Source</legend>
        <label>
          Input{' '}
          <select name="source" defaultValue={settings?.source ?? 'device'}>
            <option value="device">Device</option>
            <option value="file">File</option>
          </select>
        </label>
        <label>
          Location{' '}
          <input name="location" required defaultValue={defaultLocation} />
        </label>
        <button type="submit">Save capture settings</button>
      </fieldset>
    </form>
  );
}
