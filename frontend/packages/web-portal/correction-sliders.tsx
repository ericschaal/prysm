import type { CorrectionControls } from '@prysm/contracts/application';
import { RangeField } from './fields';
import {
  hueGuidance,
  strengthGuidance,
  whiteEnds,
  whiteGuidance,
} from './calibrate-copy';
import {
  correctionLimits,
  corrections,
  endsLabel,
  primaries,
  primaryLimits,
  signed,
  strengthEnds,
} from './calibration-model';

interface Props {
  values: CorrectionControls;
  path: string;
  change: (patch: Partial<CorrectionControls>, control: string) => void;
  onPress: () => void;
  onRelease: () => void;
  guided?: boolean;
}

export function WhiteSliders({
  values,
  path,
  change,
  guided,
  ...events
}: Props) {
  return corrections.map(([key, label, help]) => {
    if (guided && key === 'midtones') return null;
    const ends = guided && key !== 'midtones' ? whiteEnds[key] : undefined;
    return (
      <RangeField
        key={key}
        path={`${path}.${key}`}
        className={guided ? `slider-${key}` : undefined}
        label={ends ? endsLabel(ends) : label}
        ends={ends}
        help={guided && key !== 'midtones' ? whiteGuidance[key] : help}
        value={values[key]}
        onChange={(value) => change({ [key]: value }, key)}
        format={signed}
        {...correctionLimits[key]}
        {...events}
      />
    );
  });
}

export function PrimarySliders({
  primary,
  values,
  path,
  change,
  guided,
  ...events
}: Props & { primary: (typeof primaries)[number] }) {
  return (['hue', 'strength'] as const).map((key) => {
    const name = primary.name;
    const control = `${name}.${key}`;
    const ends = key === 'hue' ? primary.hue : strengthEnds;
    const help = key === 'hue' ? hueGuidance(ends) : strengthGuidance;
    return (
      <RangeField
        key={key}
        path={`${path}.${control}`}
        className={guided ? `slider-${name}-${key}` : undefined}
        label={endsLabel(ends)}
        ends={ends}
        help={guided ? help : undefined}
        value={values[name][key]}
        onChange={(value) =>
          change({ [name]: { ...values[name], [key]: value } }, control)
        }
        format={signed}
        {...primaryLimits[key]}
        {...events}
      />
    );
  });
}
