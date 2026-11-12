import type {
  DeviceSettings,
  WledSettings,
} from '@prysm/contracts/host-desktop';
import type { ApplicationRuntime } from '@prysm/application-state';
import type { State } from '@prysm/contracts/application';
import type {
  Preferences as TvPreferences,
  State as TvState,
} from '@prysm/contracts/luna';
import { WebPortal, type AdapterUis } from '@prysm/web-portal';
import { deviceCapture } from '@prysm/capture-device-ui';
import type { WebosSettings } from '@prysm/contracts/host-webos';
import { webosCapture } from '@prysm/capture-webos-ui';
import { wledOutput } from '@prysm/output-wled-ui';
import { nullOutput } from '@prysm/output-null-ui';
import type { OutputSettings as DesktopOutput } from '@prysm/contracts/host-desktop';

declare const device: ApplicationRuntime<DeviceSettings, WledSettings>;
declare const webos: ApplicationRuntime<WebosSettings, WledSettings>;

// @ts-expect-error The generated state carries revisions as numbers.
const revision: State['revision'] = '1';
// @ts-expect-error Faulted sessions name the phase that did not finish.
const faulted: State['session'] = { state: 'faulted' };
// @ts-expect-error Adapter sections are tagged by kind.
const untagged: WledSettings = { address: '192.0.2.1', leds: {} };
// Every webOS capture field is optional; Rust fills the defaults.
const defaults: WebosSettings = { kind: 'webos' };
// @ts-expect-error The webOS source has no device path.
const pathed: WebosSettings = { kind: 'webos', path: '/dev/video0' };
void [revision, faulted, untagged, defaults, pathed];

<WebPortal
  application={device}
  captures={{ device: deviceCapture }}
  outputs={{ wled: wledOutput }}
/>;
<WebPortal
  application={webos}
  captures={{ webos: webosCapture }}
  outputs={{ wled: wledOutput }}
/>;

WebPortal({
  application: webos,
  // @ts-expect-error A webOS host cannot mount the device capture panel.
  captures: { device: deviceCapture },
  outputs: { wled: wledOutput },
});
WebPortal({
  application: device,
  captures: { device: deviceCapture },
  // @ts-expect-error A capture panel cannot edit output settings.
  outputs: { wled: deviceCapture },
});
// @ts-expect-error A host must give every registered kind a panel.
const missing: AdapterUis<DesktopOutput> = { wled: wledOutput };
const outputs: AdapterUis<DesktopOutput> = { wled: wledOutput, null: nullOutput };
void [missing, outputs];

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
device.commands.configureProcessing(0, {
  brightness_percent: 80,
  shadow_lift_percent: 0,
  smoothing_seconds: 0.1,
  sample_density: 128,
  spatial_reach_percent: 10,
  // @ts-expect-error Processing fields are typed.
  remove_black_bars: 'yes',
});

// The supervisor's state is one type for both packages.
const tvState: TvState = {
  flavor: 'slim',
  preferences: { receiver: '192.168.1.20:7380' },
  enabled: true,
  lighting: null,
  power: 'active',
  video: 'available',
  worker: 'running',
  capturing: true,
  portal: null,
  failure: null,
  log: [],
};
// @ts-expect-error Only full and slim packages exist.
const otherFlavor: TvState['flavor'] = 'lite';
// @ts-expect-error Preferences are strict.
const extra: TvPreferences = { mode: 'slim' };
void [tvState, otherFlavor, extra];
