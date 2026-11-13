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
      ? `The total must be ${ledTotalLimits.min} to ${ledTotalLimits.max} LEDs. Use 0 for an edge without LEDs.`
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
            A name such as <code>wled.local</code> or an IP such as{' '}
            <code>192.168.1.50</code>, with an optional HTTP port:{' '}
            <code>wled.local:8080</code>, <code>[fe80::1]:8080</code>. WLED uses
            plain HTTP; colours always go to UDP port 4048.
          </>
        }
      />
      <fieldset className="leds" data-field="output.leds" tabIndex={-1}>
        <legend>LEDs per edge</legend>
        <p className="help">
          Seen from the front of the screen, wiring starts at the bottom-left
          corner and runs clockwise: up the left edge, across the top, down the
          right and back along the bottom. Use 0 for an edge without LEDs.
        </p>
        <div className="led-editor">
          <LedLayout leds={value.leds} />
          <div className="led-counts">
            {wiring.map(({ edge }) => (
              <NumberField
                key={edge}
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
