import type { WledSettings } from '@prysm/contracts/host-desktop';
import {
  FieldMessage,
  FindDevices,
  NumberField,
  TextField,
  useCheck,
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
  const total = ledTotal(value.leds);
  useCheck(
    'output.leds',
    total < ledTotalLimits.min || total > ledTotalLimits.max
      ? `Use ${ledTotalLimits.min} to ${ledTotalLimits.max} LEDs in total.`
      : null,
  );
  return (
    <>
      <FindDevices
        section="output"
        kind="wled"
        path="output.address"
        field="address"
        label="Find lights"
        looking="Looking for WLED lights…"
        nothing="No WLED lights found on this network."
        fallback="Enter the address below."
        onPick={(address) => onChange({ ...value, address })}
      />
      <TextField
        path="output.address"
        label="WLED address"
        required
        placeholder="wled.local"
        value={value.address}
        onChange={(address) => onChange({ ...value, address })}
        help={
          <>
            A name such as <code>wled.local</code> or an address such as{' '}
            <code>192.168.1.50</code>, with an optional port (
            <code>wled.local:8080</code>).
          </>
        }
      />
      <fieldset className="leds" data-field="output.leds" tabIndex={-1}>
        <legend>LEDs per edge</legend>
        <p className="help">
          Seen from the front, the strip starts at the bottom-left corner and
          runs clockwise. Use 0 for an edge without LEDs.
        </p>
        <div className="led-editor">
          <LedLayout leds={value.leds} />
          <div className="led-counts">
            {wiring.map(({ edge }) => (
              <NumberField
                key={edge}
                className={`led-count led-count-${edge}`}
                path={`output.leds.${edge}`}
                label={`${edgeLabels[edge]} LEDs`}
                value={value.leds[edge] ?? 0}
                onChange={(count) =>
                  onChange({ ...value, leds: { ...value.leds, [edge]: count } })
                }
                {...ledEdgeLimits}
              />
            ))}
          </div>
        </div>
        <p className="total">
          Total: <strong>{total}</strong> of at most {ledTotalLimits.max} LEDs
        </p>
        <FieldMessage path="output.leds" />
      </fieldset>
    </>
  );
}
