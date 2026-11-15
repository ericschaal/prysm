import type { WledSettings } from '@prysm/contracts/host-desktop';
import {
  FindDevices,
  NumberField,
  shownError,
  TextField,
  useCheck,
  useFieldScopeContext,
  type EditorProps,
} from '@prysm/web-portal';
import { LedLayout } from './layout';
import { ledEdgeLimits, ledTotal, ledTotalLimits, wiring } from './leds';

const edgeLabels = {
  top: 'Top',
  right: 'Right',
  bottom: 'Bottom',
  left: 'Left',
} as const;

export function WledEditor({ value, onChange }: EditorProps<WledSettings>) {
  const scope = useFieldScopeContext();
  const total = ledTotal(value.leds);
  const over = total > ledTotalLimits.max;
  useCheck(
    'output.leds',
    total < ledTotalLimits.min || over
      ? `Use ${ledTotalLimits.min} to ${ledTotalLimits.max} LEDs in total.`
      : null,
  );
  // One line under the drawing for all its fields: an edge's problem as it
  // is typed, else the total's (too many at once, too few once saved).
  const totalProblem = shownError(scope, 'output.leds', over);
  const problem =
    wiring
      .map(({ edge }) => shownError(scope, `output.leds.${edge}`, true))
      .find(Boolean) ?? totalProblem;
  return (
    <>
      <TextField
        path="output.address"
        label="WLED address"
        required
        placeholder="wled.local"
        value={value.address}
        onChange={(address) => onChange({ ...value, address })}
        help={
          <>
            Such as <code>wled.local</code> or <code>192.168.1.50</code>
          </>
        }
      />
      <FindDevices
        section="output"
        kind="wled"
        path="output.address"
        field="address"
        label="Find lights"
        looking="Looking for WLED lights…"
        nothing="No WLED lights found on this network."
        fallback="Enter the address above."
        onPick={(address) => onChange({ ...value, address })}
      />
      <fieldset className="leds" data-field="output.leds" tabIndex={-1}>
        <legend>LEDs per edge</legend>
        <LedLayout
          leds={value.leds}
          invalid={!!totalProblem}
          edges={{
            top: edgeField('top'),
            right: edgeField('right'),
            bottom: edgeField('bottom'),
            left: edgeField('left'),
          }}
        />
        <div className="field-note" data-state={problem ? 'error' : 'help'}>
          <p className="field-error" data-field-message="output.leds">
            {problem}
          </p>
        </div>
      </fieldset>
    </>
  );

  function edgeField(edge: keyof typeof edgeLabels) {
    return (
      <NumberField
        className="led-count"
        path={`output.leds.${edge}`}
        label={`${edgeLabels[edge]} LEDs`}
        value={value.leds[edge] ?? 0}
        onChange={(count) =>
          onChange({ ...value, leds: { ...value.leds, [edge]: count } })
        }
        {...ledEdgeLimits}
      />
    );
  }
}
