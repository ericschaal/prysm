import type { SettingsValues } from './api';

type Input = NonNullable<SettingsValues['input']>;
type Processing = SettingsValues['processing'];
type NumericProcessing = Exclude<keyof Processing, 'remove_black_bars'>;
export type Edge = keyof NonNullable<SettingsValues['output']>['leds'];
export type InputType = Input['type'];
export const inputLabels = {
  webos: 'LG webOS sender',
  device: 'USB capture device',
  file: 'Video file',
} satisfies Record<InputType, string>;
export type InputDraft = {
  width: string;
  height: string;
  refresh_rate_hz: string;
} & (
  | { type: 'device'; device: string }
  | { type: 'file'; path: string }
  | { type: 'webos' }
);
export type SettingsDraft = {
  calibration: SettingsValues['calibration'];
  deletedProfiles: string[];
  source: SettingsValues['source'];
  input: InputDraft;
  output: { url: string; leds: { [K in Edge]: string } };
  processing: { [K in NumericProcessing]: string } & {
    remove_black_bars: boolean;
  };
};
export type SettingsField =
  | 'source'
  | `calibration.${string}.name`
  | 'input.type'
  | 'input.device'
  | 'input.path'
  | 'input.width'
  | 'input.height'
  | 'input.refresh_rate_hz'
  | 'output.url'
  | `output.leds.${Edge}`
  | `processing.${keyof Processing}`;
export type DraftErrors = Partial<Record<SettingsField, string>>;
export const edges: readonly Edge[] = ['top', 'right', 'bottom', 'left'];
export const ledLimits = { min: 0, max: 65535, step: 1 };
const capturePreset = { width: '640', height: '360', refresh_rate_hz: '30' };
export const inputPresets = {
  device: { ...capturePreset, type: 'device', device: '/dev/video0' },
  file: { ...capturePreset, type: 'file', path: '' },
  webos: { ...capturePreset, type: 'webos' },
} as const satisfies { [K in InputType]: Extract<InputDraft, { type: K }> };
const deviceLimits = {
  width: { min: 1, max: 3840, step: 1 },
  height: { min: 1, max: 2160, step: 1 },
  refresh_rate_hz: { min: 1, max: 4294967295, step: 1 },
};
const captureLimits = {
  device: deviceLimits,
  file: deviceLimits,
  webos: {
    width: { min: 2, max: 1920, step: 2 },
    height: { min: 2, max: 1080, step: 2 },
    refresh_rate_hz: { min: 1, max: 60, step: 1 },
  },
};
export const inputLimits = (type: InputType) => captureLimits[type];
export const numericProcessing = [
  {
    key: 'brightness_percent',
    label: 'Brightness',
    min: 0,
    max: 100,
    step: 1,
    help: 'Output brightness, from 0 to 100 percent.',
  },
  {
    key: 'shadow_lift_percent',
    label: 'Shadow lift',
    min: 0,
    max: 100,
    step: 1,
    help: 'Lift dark colors while keeping black off.',
  },
  {
    key: 'smoothing_seconds',
    label: 'Smoothing duration',
    min: 0,
    max: 3.4028234663852886e38,
    step: 'any',
    help: 'Seconds to soften a color change. Use 0 to disable smoothing.',
  },
  {
    key: 'sample_density',
    label: 'Sample density',
    min: 1,
    max: 4294967295,
    step: 1,
    help: 'Samples per 1000 pixels. Start at 128; higher values use more CPU and memory.',
  },
  {
    key: 'spatial_reach_percent',
    label: 'Spatial reach',
    min: 1,
    max: 100,
    step: 1,
    help: 'How far inside the picture to sample, from 1 to 100 percent.',
  },
] as const satisfies readonly {
  key: NumericProcessing;
  label: string;
  min: number;
  max: number;
  step: number | 'any';
  help: string;
}[];
export const equal = (a: unknown, b: unknown) =>
  JSON.stringify(a) === JSON.stringify(b);

export function inputDraft(input: SettingsValues['input']): InputDraft {
  if (!input) return { ...inputPresets.webos };
  const common = {
    width: String(input.capture_resolution[0]),
    height: String(input.capture_resolution[1]),
    refresh_rate_hz: String(input.refresh_rate_hz),
  };
  if (input.type === 'device')
    return { ...common, type: input.type, device: input.device };
  if (input.type === 'file')
    return { ...common, type: input.type, path: input.path };
  return { ...common, type: input.type };
}

export function changeInputType(
  current: InputDraft,
  type: InputType,
  saved: SettingsValues['input'],
): InputDraft {
  const preset = saved?.type === type ? inputDraft(saved) : inputPresets[type];
  return {
    ...preset,
    width: current.width,
    height: current.height,
    refresh_rate_hz: current.refresh_rate_hz,
  };
}

export function draftFrom(settings: SettingsValues): SettingsDraft {
  const { processing, output } = settings;
  return {
    calibration: settings.calibration,
    deletedProfiles: [],
    source: settings.source,
    input: inputDraft(settings.input),
    output: {
      url: output?.url ?? '',
      leds: {
        top: String(output?.leds.top ?? 0),
        right: String(output?.leds.right ?? 0),
        bottom: String(output?.leds.bottom ?? 0),
        left: String(output?.leds.left ?? 0),
      },
    },
    processing: {
      brightness_percent: String(processing.brightness_percent),
      shadow_lift_percent: String(processing.shadow_lift_percent),
      smoothing_seconds: String(processing.smoothing_seconds),
      sample_density: String(processing.sample_density),
      spatial_reach_percent: String(processing.spatial_reach_percent),
      remove_black_bars: processing.remove_black_bars,
    },
  };
}

export function validate(draft: SettingsDraft): DraftErrors {
  const errors: DraftErrors = {};
  const required = (field: SettingsField, value: string) => {
    if (!value.trim()) errors[field] = 'Enter a value.';
  };
  const number = (
    field: SettingsField,
    text: string,
    min: number,
    max: number,
    integer = true,
  ) => {
    const value = Number(text);
    if (
      !text.trim() ||
      !Number.isFinite(value) ||
      value < min ||
      value > max ||
      (integer && !Number.isInteger(value))
    )
      errors[field] = integer
        ? `Enter a whole number from ${min} to ${max}.`
        : 'Enter a non-negative number within the supported range.';
  };
  const { input, output, processing } = draft;
  if (input.type === 'device') required('input.device', input.device);
  else if (input.type === 'file') required('input.path', input.path);
  const limits = inputLimits(input.type);
  for (const dimension of ['width', 'height'] as const) {
    const { min, max, step } = limits[dimension];
    number(`input.${dimension}`, input[dimension], min, max);
    if (step > 1 && Number(input[dimension]) % step)
      errors[`input.${dimension}`] = 'webOS dimensions must be even numbers.';
  }
  number(
    'input.refresh_rate_hz',
    input.refresh_rate_hz,
    limits.refresh_rate_hz.min,
    limits.refresh_rate_hz.max,
  );
  try {
    const url = new URL(output.url);
    if (
      !['http:', 'https:'].includes(url.protocol) ||
      url.username ||
      url.password ||
      url.pathname !== '/' ||
      url.search ||
      url.hash
    )
      throw Error();
  } catch {
    errors['output.url'] =
      'Enter an HTTP(S) base URL, such as http://192.168.1.20/, without a path or credentials.';
  }
  for (const edge of edges)
    number(
      `output.leds.${edge}`,
      output.leds[edge],
      ledLimits.min,
      ledLimits.max,
    );
  const total = edges.reduce((sum, edge) => sum + Number(output.leds[edge]), 0);
  if (total < 1 || total > ledLimits.max)
    errors['output.leds.top'] =
      `The total LED count must be from 1 to ${ledLimits.max}. Use 0 for an absent edge.`;
  for (const field of numericProcessing)
    number(
      `processing.${field.key}`,
      processing[field.key],
      field.min,
      field.max,
      field.step !== 'any',
    );
  for (const { id, name } of draft.calibration.profiles) {
    if (draft.deletedProfiles.includes(id)) continue;
    if (
      !name.trim() ||
      name !== name.trim() ||
      [...name].length > 64 ||
      /\p{Cc}/u.test(name)
    )
      errors[`calibration.${id}.name`] =
        'Use 1–64 characters without surrounding spaces or control characters.';
  }
  return errors;
}

function inputValue(draft: InputDraft): Input {
  const common = {
    capture_resolution: [Number(draft.width), Number(draft.height)] as [
      number,
      number,
    ],
    refresh_rate_hz: Number(draft.refresh_rate_hz),
  };
  if (draft.type === 'device')
    return { type: draft.type, device: draft.device, ...common };
  if (draft.type === 'file')
    return { type: draft.type, path: draft.path, ...common };
  return { type: draft.type, ...common };
}

export function settingsFrom(draft: SettingsDraft): SettingsValues {
  const { leds, url } = draft.output;
  const p = draft.processing;
  return {
    calibration: {
      active: draft.deletedProfiles.includes(draft.calibration.active ?? '')
        ? null
        : draft.calibration.active,
      profiles: draft.calibration.profiles.filter(
        (p) => !draft.deletedProfiles.includes(p.id),
      ),
    },
    source: draft.source,
    input: inputValue(draft.input),
    output: {
      type: 'wled',
      url,
      leds: {
        top: Number(leds.top),
        right: Number(leds.right),
        bottom: Number(leds.bottom),
        left: Number(leds.left),
      },
    },
    processing: {
      brightness_percent: Number(p.brightness_percent),
      shadow_lift_percent: Number(p.shadow_lift_percent),
      smoothing_seconds: Number(p.smoothing_seconds),
      sample_density: Number(p.sample_density),
      spatial_reach_percent: Number(p.spatial_reach_percent),
      remove_black_bars: p.remove_black_bars,
    },
  };
}
