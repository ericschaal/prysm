import type {
  DeviceSettings,
  WledSettings,
} from '@prysm/contracts/host-desktop';
import type { ApplicationRuntime } from '@prysm/application-state';
import type { State } from '@prysm/contracts/application';
import { WebPortal } from '@prysm/web-portal';
import { DeviceCaptureSettings } from '@prysm/capture-device-ui';
import {
  WebosCaptureSettings,
  type WebosCaptureConfig,
} from '@prysm/capture-webos-ui';
import { WledOutputSettings } from '@prysm/output-wled-ui';

declare const device: ApplicationRuntime<DeviceSettings, WledSettings>;
declare const webos: ApplicationRuntime<WebosCaptureConfig, WledSettings>;

// @ts-expect-error The generated state carries revisions as numbers.
const revision: State['revision'] = '1';
// @ts-expect-error Faulted sessions name the phase that did not finish.
const faulted: State['session'] = { state: 'faulted' };
// @ts-expect-error Adapter sections are tagged by kind.
const untagged: WledSettings = { address: '192.0.2.1', leds: {} };
void [revision, faulted, untagged];

<WebPortal
  application={device}
  captureSettings={DeviceCaptureSettings}
  outputSettings={WledOutputSettings}
/>;
<WebPortal
  application={webos}
  captureSettings={WebosCaptureSettings}
  outputSettings={WledOutputSettings}
/>;

WebPortal({
  application: webos,
  // @ts-expect-error A webOS host cannot mount the device capture panel.
  captureSettings: DeviceCaptureSettings,
  outputSettings: WledOutputSettings,
});
WebPortal({
  application: device,
  captureSettings: DeviceCaptureSettings,
  // @ts-expect-error A capture panel cannot edit output settings.
  outputSettings: DeviceCaptureSettings,
});

// @ts-expect-error The host's typed thunk rejects another adapter's settings.
device.commands.configureCapture(0, { kind: 'webos', fps: 30 });
webos.commands.configureCapture(0, {
  // @ts-expect-error Output settings cannot be submitted as capture settings.
  kind: 'wled',
  address: '192.0.2.1',
  leds: {},
});
// @ts-expect-error Revisions are numbers, not decimal strings.
device.commands.configureOutput('0', {
  kind: 'wled',
  address: '192.0.2.1',
  leds: {},
});
