import { useState } from 'react';
import type { SettingsPanelProps } from '@prysm/web-portal';
import type { ServerCaptureSettings as ServerCaptureConfig } from '@prysm/contracts/host-server';
import { DesktopCaptureSettings } from '@prysm/capture-desktop-ui';
import { RemoteCaptureSettings } from '@prysm/remote-capture-ui';

export function ServerCaptureSettings(
  props: SettingsPanelProps<ServerCaptureConfig>,
) {
  return (
    <CaptureEditor key={props.settings?.adapter ?? 'desktop'} {...props} />
  );
}

function CaptureEditor({
  settings,
  disabled,
  onSubmit,
}: SettingsPanelProps<ServerCaptureConfig>) {
  const [adapter, setAdapter] = useState<ServerCaptureConfig['adapter']>(
    settings?.adapter ?? 'desktop',
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
            if (value === 'desktop' || value === 'remote') setAdapter(value);
          }}
        >
          <option value="desktop">Desktop</option>
          <option value="remote">Remote</option>
        </select>
      </label>
      {adapter === 'desktop' ? (
        <DesktopCaptureSettings
          settings={settings?.adapter === 'desktop' ? settings.settings : null}
          disabled={disabled}
          onSubmit={(value) =>
            onSubmit({ adapter: 'desktop', settings: value })
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
