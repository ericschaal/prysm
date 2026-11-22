import type {
  DeviceSettings,
  WledSettings,
} from '@prysm/contracts/host-desktop';
import type { CaptureModesClient } from '@prysm/application-client';
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
declare const captureModes: CaptureModesClient;

// @ts-expect-error The generated state carries revisions as numbers.
const revision: State['revision'] = '1';
// @ts-expect-error Faulted sessions say what did not finish.
const faulted: State['session'] = { state: 'faulted' };
const syncFailure: State['session'] = {
  state: 'screen_sync',
  // @ts-expect-error Screen sync carries no failure.
  failure: { kind: 'internal', message: 'x' },
};
const stoppedFailure: State['session'] = {
  state: 'stopped',
  failure: { kind: 'adapter', message: 'x', section: 'output' },
};
// @ts-expect-error Adapter sections are tagged by kind.
const untagged: WledSettings = { address: '192.0.2.1', leds: {} };
// Every webOS capture field is optional; Rust fills the defaults.
const defaults: WebosSettings = { kind: 'webos' };
// @ts-expect-error The webOS source has no device path.
const pathed: WebosSettings = { kind: 'webos', path: '/dev/video0' };
void [
  revision,
  faulted,
  syncFailure,
  stoppedFailure,
  untagged,
  defaults,
  pathed,
];

<WebPortal
  application={device}
  captures={{ device: deviceCapture }}
  outputs={{ wled: wledOutput }}
  captureModes={captureModes}
/>;
<WebPortal
  application={webos}
  captures={{ webos: webosCapture }}
  outputs={{ wled: wledOutput }}
  captureModes={captureModes}
/>;
// @ts-expect-error Every host says what its captures deliver.
<WebPortal
  application={device}
  captures={{ device: deviceCapture }}
  outputs={{ wled: wledOutput }}
/>;

WebPortal({
  application: webos,
  captureModes,
  // @ts-expect-error A webOS host cannot mount the device capture panel.
  captures: { device: deviceCapture },
  outputs: { wled: wledOutput },
});
WebPortal({
  application: device,
  captureModes,
  captures: { device: deviceCapture },
  // @ts-expect-error A capture panel cannot edit output settings.
  outputs: { wled: deviceCapture },
});
// @ts-expect-error A host must give every registered kind a panel.
const missing: AdapterUis<DesktopOutput> = { wled: wledOutput };
const outputs: AdapterUis<DesktopOutput> = {
  wled: wledOutput,
  null: nullOutput,
};
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
  style: { kind: 'natural' },
  sample_density: 128,
  // @ts-expect-error Processing fields are typed.
  remove_black_bars: 'yes',
});
device.commands.configureProcessing(0, {
  brightness_percent: 80,
  // @ts-expect-error A style is one of the named kinds.
  style: { kind: 'cinema' },
  sample_density: 128,
  remove_black_bars: true,
});

// The supervisor's state is one type for both packages, tagged by flavor:
// each package has only its own fields.
const tvState: TvState = {
  flavor: 'slim',
  preferences: { prysm_address: 'http://192.168.1.20:8787' },
  enabled: true,
  power: 'active',
  video: 'available',
  worker: {
    phase: 'running',
    capturing: true,
    link: { state: 'sending', encoding: 'raw', fps: 30 },
  },
  log: [],
};
const fullState: TvState = {
  flavor: 'full',
  preferences: {},
  target: 'screen_sync',
  power: 'active',
  video: 'available',
  worker: { phase: 'stopping', portal: 'http://192.168.1.5:8787/' },
  log: [],
};
const fullOnly: TvState = {
  flavor: 'slim',
  preferences: {},
  enabled: true,
  power: 'active',
  video: 'available',
  // @ts-expect-error The slim worker has no portal.
  worker: { phase: 'running', capturing: true, paused: false, portal: 'x' },
  log: [],
};
const slimOnly: TvState = {
  flavor: 'full',
  preferences: {},
  // @ts-expect-error The full package's intent is its target, not `enabled`.
  enabled: true,
  target: 'off',
  power: 'active',
  video: 'available',
  worker: { phase: 'stopped' },
  log: [],
};
const nullFailure: TvState = {
  ...fullState,
  // @ts-expect-error An absent failure is omitted, never `null`.
  failure: null,
};
// @ts-expect-error A restart says when.
const retrying: TvState['worker'] = { phase: 'retrying' };
// @ts-expect-error Only full and slim packages exist.
const otherFlavor: TvState['flavor'] = 'lite';
// @ts-expect-error Preferences are strict.
const extra: TvPreferences = { mode: 'slim' };
void [
  tvState,
  fullState,
  fullOnly,
  slimOnly,
  nullFailure,
  retrying,
  otherFlavor,
  extra,
];
