import type { WledSettings } from '@prysm/contracts/host-desktop';
import type { SettingsPanelProps } from '@prysm/web-portal';

/**
 * `host[:port]` from what a user typed or pasted: surrounding spaces, a leading
 * `http://` and a trailing `/` are dropped. Everything else, including
 * `https://`, is left for the server to accept or explain.
 */
export function normalizeWledAddress(text: string): string {
  const trimmed = text.trim();
  const bare = /^http:\/\//i.test(trimmed) ? trimmed.slice('http://'.length) : trimmed;
  return bare.endsWith('/') ? bare.slice(0, -1) : bare;
}

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
          address: normalizeWledAddress(address),
          leds: settings?.leds ?? {},
        });
      }}
    >
      <h2>WLED output</h2>
      <fieldset disabled={disabled}>
        <legend>Device</legend>
        <label>
          Host or IP address{' '}
          <input
            name="address"
            required
            autoComplete="off"
            autoCapitalize="off"
            spellCheck={false}
            placeholder="wled.local"
            aria-describedby="wled-address-help"
            defaultValue={settings?.address ?? ''}
          />
        </label>
        <p id="wled-address-help">
          A name such as <code>wled.local</code> or an IP such as <code>192.168.1.50</code>, with an
          optional HTTP port: <code>wled.local:8080</code>, <code>[fe80::1]:8080</code>. WLED uses
          plain HTTP; colours always go to UDP port 4048.
        </p>
        <button type="submit">Save output settings</button>
      </fieldset>
    </form>
  );
}
