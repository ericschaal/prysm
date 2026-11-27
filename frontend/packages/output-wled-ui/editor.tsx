import { useId, useState } from 'react';
import type { WledSettings } from '@prysm/contracts/host-desktop';
import {
  Advanced,
  CheckboxField,
  DeviceRows,
  LedDrawing,
  NumberField,
  shownError,
  useCheck,
  useFieldScopeContext,
  type EditorProps,
} from '@prysm/web-portal';
import { normalizeWledAddress } from './address';
import {
  ditherDefault,
  ledEdgeLimits,
  ledEdges,
  ledTotal,
  ledTotalLimits,
  ledTotalProblem,
  orderCaption,
  wiring,
} from './leds';

const edgeLabels = {
  top: 'Top',
  right: 'Right',
  bottom: 'Bottom',
  left: 'Left',
} as const;

const typedAddress = {
  choice: 'Other lights',
  label: 'Address',
  placeholder: 'wled.local',
  empty: 'Choose your lights, or enter their address.',
};

/**
 * The lights as rows from discovery, the last one typed, then the LEDs on
 * each edge of the screen.
 */
export function WledEditor({ value, onChange }: EditorProps<WledSettings>) {
  const scope = useFieldScopeContext();
  const legendId = useId();
  /** The found lights last picked, with the LED count they gave. */
  const [picked, setPicked] = useState<{ address: string; leds: number }>();
  const leds = ledEdges(value.leds);
  const total = ledTotal(leds);
  // The address as the rows compare it: `http://wled.local:80/` is theirs.
  const address = normalizeWledAddress(value.address);
  // Said under the drawing while the address is still the picked one's;
  // the edges stay the person's to fill.
  const found = picked?.address === address ? picked.leds : undefined;
  const over = total > ledTotalLimits.max;
  useCheck(
    'output.leds',
    total < ledTotalLimits.min || over ? ledTotalProblem : null,
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
      <DeviceRows
        section="output"
        kind="wled"
        field="address"
        legend="Lights"
        things="lights"
        nothing="No WLED lights found on this network. Enter their address."
        value={address}
        text={value.address}
        typed={typedAddress}
        onPick={(next, candidate) => {
          setPicked(
            candidate.led_count === undefined
              ? undefined
              : { address: next, leds: candidate.led_count },
          );
          onChange({ ...value, address: next });
        }}
        onType={(typed) => onChange({ ...value, address: typed })}
      />
      <fieldset
        className="leds"
        aria-labelledby={legendId}
        data-field="output.leds"
        tabIndex={-1}
      >
        {/* Named by its label; the side it is seen from is an aside. */}
        <legend>
          <span id={legendId}>LEDs per edge</span>
          <span>Seen from the front</span>
        </legend>
        <LedDrawing
          edges={leds}
          caption={orderCaption(leds)}
          editing={{
            wiring,
            invalid: !!totalProblem,
            counts: {
              top: edgeField('top'),
              right: edgeField('right'),
              bottom: edgeField('bottom'),
              left: edgeField('left'),
            },
          }}
        />
        <div className="field-note" data-state={problem ? 'error' : 'help'}>
          <p
            className="help"
            data-tone={
              found !== undefined && found !== total ? 'warning' : undefined
            }
          >
            {found === undefined ? null : `This WLED has ${found} LEDs.`}
          </p>
          <p className="field-error" data-field-message="output.leds">
            {problem}
          </p>
        </div>
      </fieldset>
      <Advanced paths={['output.dither']}>
        <CheckboxField
          path="output.dither"
          label="Smooth dark colours"
          help="Mixes neighbouring LED levels so dim colours fade evenly. Turn off if dark scenes twinkle."
          checked={value.dither ?? ditherDefault}
          onChange={(dither) => onChange({ ...value, dither })}
        />
      </Advanced>
    </>
  );

  function edgeField(edge: keyof typeof edgeLabels) {
    return (
      <NumberField
        className="led-count"
        path={`output.leds.${edge}`}
        label={`${edgeLabels[edge]} LEDs`}
        value={leds[edge]}
        onChange={(count) =>
          onChange({ ...value, leds: { ...value.leds, [edge]: count } })
        }
        {...ledEdgeLimits}
      />
    );
  }
}
