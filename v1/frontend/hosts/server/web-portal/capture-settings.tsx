import { useState } from 'react';
import type { SettingsPanelProps } from '@prysm/web-portal';
import type { ServerCaptureSettings as ServerCaptureConfig } from '@prysm/contracts/host-server';
import { DeviceCaptureSettings } from '@prysm/capture-device-ui';
import { RemoteCaptureSettings } from '@prysm/remote-capture-ui';

export function ServerCaptureSettings(
  props: SettingsPanelProps<ServerCaptureConfig>,
) {
  return (
    <CaptureEditor key={props.settings?.adapter ?? 'device'} {...props} />
  );
}

function CaptureEditor({
  settings,
  disabled,
  onSubmit,
}: SettingsPanelProps<ServerCaptureConfig>) {
  const [adapter, setAdapter] = useState<ServerCaptureConfig['adapter']>(
    settings?.adapter ?? 'device',
  );
  return (
    <section>
      <label>
        Capture adapter{' '}
        <select
          name="adapter"
          disabled={disabled}
          value={adapter}
          onChange={(event) => {
            const value = event.currentTarget.value;
            if (value === 'device' || value === 'remote') setAdapter(value);
          }}
        >
          <option value="device">Device</option>
          <option value="remote">Remote</option>
        </select>
      </label>
      {adapter === 'device' ? (
        <DeviceCaptureSettings
          settings={settings?.adapter === 'device' ? settings.settings : null}
          disabled={disabled}
          onSubmit={(value) =>
            onSubmit({ adapter: 'device', settings: value })
          }
        />
      ) : (
        <RemoteCaptureSettings
          settings={settings?.adapter === 'remote' ? settings.settings : null}
          disabled={disabled}
          onSubmit={(value) => onSubmit({ adapter: 'remote', settings: value })}
        />
      )}
    </section>
  );
}
