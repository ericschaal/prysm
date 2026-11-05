import type { SettingsPanelProps } from '@prysm/web-portal';
import type { RemoteCaptureSettings as RemoteCaptureConfig } from '@prysm/contracts/remote-capture';

export function RemoteCaptureSettings({
  settings,
  disabled,
  onSubmit,
}: SettingsPanelProps<RemoteCaptureConfig>) {
  return (
    <form
      key={JSON.stringify(settings)}
      onSubmit={(event) => {
        event.preventDefault();
        const data = new FormData(event.currentTarget);
        const peer = data.get('peer');
        if (typeof peer !== 'string') return;
        void onSubmit({ peer });
      }}
    >
      <h2>Remote capture</h2>
      <fieldset disabled={disabled}>
        <legend>Connection</legend>
        <label>
          Peer{' '}
          <input name="peer" required defaultValue={settings?.peer ?? ''} />
        </label>
        <button type="submit">Save capture settings</button>
      </fieldset>
    </form>
  );
}
