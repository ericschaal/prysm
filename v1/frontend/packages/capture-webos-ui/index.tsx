import type { SettingsPanelProps } from '@prysm/web-portal';

import type { TvCaptureSettings as WebosCaptureConfig } from '@prysm/contracts/webos';

/** Minimal composition panel; native resolution controls remain deferred. */
export function WebosCaptureSettings({
  settings,
  disabled,
  onSubmit,
}: SettingsPanelProps<WebosCaptureConfig>) {
  return (
    <form
      key={JSON.stringify(settings)}
      onSubmit={(event) => {
        event.preventDefault();
        const data = new FormData(event.currentTarget);
        void onSubmit({
          resolution: settings?.resolution ?? [320, 180],
          rate: Number(data.get('rate')),
        });
      }}
    >
      <h2>webOS capture</h2>
      <fieldset disabled={disabled}>
        <legend>Capture</legend>
        <label>
          Frames per second{' '}
          <input
            name="rate"
            type="number"
            min="1"
            max="120"
            step="1"
            required
            defaultValue={settings?.rate ?? 30}
          />
        </label>
        <button type="submit">Save capture settings</button>
      </fieldset>
    </form>
  );
}
