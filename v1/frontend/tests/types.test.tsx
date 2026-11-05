import type { WledSettings as WledOutputConfig } from '@prysm/contracts/wled';
import type { TvCaptureSettings as WebosCaptureConfig } from '@prysm/contracts/webos';
import type { RemoteCaptureSettings as RemoteCaptureConfig } from '@prysm/contracts/remote-capture';
import type { DesktopCaptureSettings as DesktopCaptureConfig } from '@prysm/contracts/desktop';
import { WebPortal } from '@prysm/web-portal';
import type { ApplicationRuntime } from '@prysm/application-state';
import { DesktopCaptureSettings } from '@prysm/capture-desktop-ui';
import { WebosCaptureSettings } from '@prysm/capture-webos-ui';
import { WledOutputSettings } from '@prysm/output-wled-ui';

declare const desktop: ApplicationRuntime<
  DesktopCaptureConfig,
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
// @ts-expect-error Desktop capture only selects local device/file sources.
const remoteDesktop: DesktopCaptureConfig['source'] = 'remote';
void remoteDesktop;
void [shortResolution, shortLeds, remote];

<WebPortal
  application={desktop}
  captureSettings={DesktopCaptureSettings}
  outputSettings={WledOutputSettings}
/>;
<WebPortal
  application={webos}
  captureSettings={WebosCaptureSettings}
  outputSettings={WledOutputSettings}
/>;

WebPortal({
  application: webos,
  // @ts-expect-error A webOS client cannot supply the desktop panel's source/path settings.
  captureSettings: DesktopCaptureSettings,
  outputSettings: WledOutputSettings,
});
WebPortal({
  application: desktop,
  captureSettings: DesktopCaptureSettings,
  // @ts-expect-error Capture settings cannot be used as WLED output settings.
  outputSettings: DesktopCaptureSettings,
});

// @ts-expect-error The host's typed thunk rejects a different adapter's settings.
desktop.commands.configureCapture('0', { rate: 30, resolution: [320, 180] });
webos.commands.configureCapture('0', {
  // @ts-expect-error Output settings cannot be submitted to the capture command.
  address: '192.0.2.1',
  leds: [1, 2, 3, 4],
});
