import { useState } from 'react';
import type { WledSettings } from '@prysm/contracts/host-desktop';
import {
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
  ledEdgeLimits,
  ledEdges,
  ledRuns,
  ledTotal,
  ledTotalLimits,
  wiring,
  type LedRun,
} from './leds';

const edgeLabels = {
  top: 'Top',
  right: 'Right',
  bottom: 'Bottom',
  left: 'Left',
} as const;

const describeRun = (run: LedRun) =>
  run.first === undefined
    ? `${run.edge}: none`
    : `${run.edge} ${run.direction}: LEDs ${run.first}–${run.last}`;

/** Where LED 1 is and the order of every edge, for screen readers. */
function orderCaption(leds: WledSettings['leds']) {
  const runs = ledRuns(leds);
  const start = runs.find((run) => run.count > 0);
  return start
    ? `LED 1 is at the ${start.from} corner. Order: ${runs.map(describeRun).join('; ')}.`
    : 'No LEDs yet.';
}

const typedAddress = {
  choice: 'Other lights',
  label: 'Address',
  placeholder: 'wled.local',
  empty: 'Choose your lights, or enter their address by hand.',
};

/**
 * The lights as rows from discovery, the last one typed, then the LEDs on
 * each edge of the screen.
 */
export function WledEditor({ value, onChange }: EditorProps<WledSettings>) {
  const scope = useFieldScopeContext();
  /** The found lights last picked, with the LED count they gave. */
  const [picked, setPicked] = useState<{ address: string; leds: number }>();
  const total = ledTotal(value.leds);
  // The address as the rows compare it: `http://wled.local:80/` is theirs.
  const address = normalizeWledAddress(value.address);
  // Said under the drawing while the address is still the picked one's;
  // the edges stay the person's to fill.
  const found = picked?.address === address ? picked.leds : undefined;
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
    <DeviceRows
      section="output"
      kind="wled"
      path="output.address"
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
      then={
        <fieldset className="leds" data-field="output.leds" tabIndex={-1}>
          <legend>LEDs per edge, seen from the front</legend>
          <LedDrawing
            edges={ledEdges(value.leds)}
            caption={orderCaption(value.leds)}
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
      }
      advanced={{
        paths: ['output.dither'],
        fields: (
          <CheckboxField
            path="output.dither"
            label="Smooth dark colours"
            help="Mixes neighbouring LED levels so dim colours fade evenly. Turn off if dark scenes twinkle."
            checked={value.dither ?? true}
            onChange={(dither) => onChange({ ...value, dither })}
          />
        ),
      }}
    />
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
