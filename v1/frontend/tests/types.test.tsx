import type { WledSettings as WledOutputConfig } from '@prysm/contracts/wled';
import type { TvCaptureSettings as WebosCaptureConfig } from '@prysm/contracts/webos';
import type { RemoteCaptureSettings as RemoteCaptureConfig } from '@prysm/contracts/remote-capture';
import type { DeviceCaptureSettings as DeviceCaptureConfig } from '@prysm/contracts/device';
import { WebPortal } from '@prysm/web-portal';
import type { ApplicationRuntime } from '@prysm/application-state';
import { DeviceCaptureSettings } from '@prysm/capture-device-ui';
import { WebosCaptureSettings } from '@prysm/capture-webos-ui';
import { WledOutputSettings } from '@prysm/output-wled-ui';

declare const device: ApplicationRuntime<
  DeviceCaptureConfig,
  WledOutputConfig
>;
declare const webos: ApplicationRuntime<WebosCaptureConfig, WledOutputConfig>;

// @ts-expect-error Resolution is a fixed pair generated from the Rust wire contract.
const shortResolution: WebosCaptureConfig['resolution'] = [320];
// @ts-expect-error WLED requires exactly four edge counts.
const shortLeds: WledOutputConfig['leds'] = [1, 2, 3];
const remote: RemoteCaptureConfig = {
  peer: '127.0.0.1:9000',
  // @ts-expect-error The transport does not own the producing source's resolution.
  resolution: [320, 180],
};
// @ts-expect-error Video device capture only selects local device/file sources.
const remoteDevice: DeviceCaptureConfig['source'] = 'remote';
void remoteDevice;
void [shortResolution, shortLeds, remote];

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
  // @ts-expect-error A webOS client cannot supply the device panel's source/path settings.
  captureSettings: DeviceCaptureSettings,
  outputSettings: WledOutputSettings,
});
WebPortal({
  application: device,
  captureSettings: DeviceCaptureSettings,
  // @ts-expect-error Capture settings cannot be used as WLED output settings.
  outputSettings: DeviceCaptureSettings,
});

// @ts-expect-error The host's typed thunk rejects a different adapter's settings.
device.commands.configureCapture('0', { rate: 30, resolution: [320, 180] });
webos.commands.configureCapture('0', {
  // @ts-expect-error Output settings cannot be submitted to the capture command.
  address: '192.0.2.1',
  leds: [1, 2, 3, 4],
});
