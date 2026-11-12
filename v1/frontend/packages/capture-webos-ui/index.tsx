import type { SettingsPanelProps } from '@prysm/web-portal';

/**
 * Provisional webOS capture settings under kind `webos`, written by hand: the
 * adapter is unfinished; use `host-webos`'s generated type once it has a schema.
 */
export interface WebosCaptureConfig {
  kind: 'webos';
  fps: number;
}

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
        const fps = Number(new FormData(event.currentTarget).get('fps'));
        void onSubmit({ ...settings, kind: 'webos', fps });
      }}
    >
      <h2>webOS capture</h2>
      <fieldset disabled={disabled}>
        <legend>Capture</legend>
        <label>
          Frames per second{' '}
          <input
            name="fps"
            type="number"
            min="1"
            max="120"
            step="1"
            required
            defaultValue={settings?.fps ?? 30}
          />
        </label>
        <button type="submit">Save capture settings</button>
      </fieldset>
    </form>
  );
}
