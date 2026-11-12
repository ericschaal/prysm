import type { CaptureSettings } from '@prysm/contracts/host-desktop';
import { useState } from 'react';
import type { SettingsPanelProps } from '@prysm/web-portal';
import { DeviceCaptureSettings } from '@prysm/capture-device-ui';
import { RemoteCaptureSettings } from '@prysm/remote-capture-ui';

/** The capture sources the desktop hosts register, by kind. */
export type HeadlessCaptureConfig = CaptureSettings;

/** Choosing a kind only switches the form; saving sends the new section. */
export function HeadlessCaptureSettings(
  props: SettingsPanelProps<HeadlessCaptureConfig>,
) {
  return <CaptureEditor key={props.settings?.kind ?? 'device'} {...props} />;
}

function CaptureEditor({
  settings,
  disabled,
  onSubmit,
}: SettingsPanelProps<HeadlessCaptureConfig>) {
  const [kind, setKind] = useState<HeadlessCaptureConfig['kind']>(
    settings?.kind ?? 'device',
  );
  return (
    <section>
      <label>
        Capture source{' '}
        <select
          name="kind"
          disabled={disabled}
          value={kind}
          onChange={(event) => {
            const value = event.currentTarget.value;
            if (value === 'device' || value === 'remote') setKind(value);
          }}
        >
          <option value="device">Device</option>
          <option value="remote">Remote</option>
        </select>
      </label>
      {kind === 'device' ? (
        <DeviceCaptureSettings
          settings={settings?.kind === 'device' ? settings : null}
          disabled={disabled}
          onSubmit={onSubmit}
        />
      ) : (
        <RemoteCaptureSettings
          settings={settings?.kind === 'remote' ? settings : null}
          disabled={disabled}
          onSubmit={onSubmit}
        />
      )}
    </section>
  );
}
