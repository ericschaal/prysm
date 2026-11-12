import type { RemoteReceiverSettings } from '@prysm/contracts/host-desktop';
import type { SettingsPanelProps } from '@prysm/web-portal';

export function RemoteCaptureSettings({
  settings,
  disabled,
  onSubmit,
}: SettingsPanelProps<RemoteReceiverSettings>) {
  return (
    <form
      key={JSON.stringify(settings)}
      onSubmit={(event) => {
        event.preventDefault();
        const listen = new FormData(event.currentTarget).get('listen');
        if (typeof listen !== 'string') return;
        void onSubmit({ kind: 'remote', listen });
      }}
    >
      <h2>Remote capture</h2>
      <fieldset disabled={disabled}>
        <legend>Receiver</legend>
        <label>
          Listen address{' '}
          <input name="listen" required defaultValue={settings?.listen ?? ''} />
        </label>
        <button type="submit">Save capture settings</button>
      </fieldset>
    </form>
  );
}
