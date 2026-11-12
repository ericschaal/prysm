import type { WledSettings } from '@prysm/contracts/host-desktop';
import {
  FieldMessage,
  NumberField,
  TextField,
  useCheck,
  type AdapterUi,
  type EditorProps,
} from '@prysm/web-portal';
import {
  LedLayout,
  ledEdgeLimits,
  ledTotal,
  ledTotalLimits,
  wiring,
} from './layout';

export {
  LedLayout,
  ledEdgeLimits,
  ledRuns,
  ledTotal,
  ledTotalLimits,
  wiring,
} from './layout';

/**
 * `host[:port]` from what a user typed or pasted: surrounding spaces, a leading
 * `http://` and a trailing `/` are dropped. Everything else, including
 * `https://`, is left for the server to accept or explain.
 */
export function normalizeWledAddress(text: string): string {
  const trimmed = text.trim();
  const bare = /^http:\/\//i.test(trimmed) ? trimmed.slice('http://'.length) : trimmed;
  return bare.endsWith('/') ? bare.slice(0, -1) : bare;
}

/**
 * WLED's own web interface for a saved `address` (`host`, `host:port`,
 * `[ipv6]:port` or a bare IPv6 address), or `null` when it is not one.
 */
export function wledUrl(address: string): string | null {
  const text = normalizeWledAddress(address);
  if (!text || /[\s/?#@\\]/.test(text)) return null;
  // A bare IPv6 address has more than one colon and needs brackets in a URL.
  const authority =
    !text.startsWith('[') && text.indexOf(':') !== text.lastIndexOf(':')
      ? `[${text}]`
      : text;
  try {
    return new URL(`http://${authority}/`).href;
  } catch {
    return null;
  }
}

const edgeLabels = {
  top: 'Top',
  right: 'Right',
  bottom: 'Bottom',
  left: 'Left',
} as const;

function WledEditor({ value, onChange }: EditorProps<WledSettings>) {
  const total = ledTotal(value.leds);
  useCheck(
    'output.leds',
    total < ledTotalLimits.min || total > ledTotalLimits.max
      ? `The total must be ${ledTotalLimits.min} to ${ledTotalLimits.max} LEDs. Use 0 for an edge without LEDs.`
      : null,
  );
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

export const wledOutput: AdapterUi<WledSettings> = {
  label: 'WLED',
  create: () => ({ kind: 'wled', address: '', leds: {} }),
  Editor: WledEditor,
  normalize: (value) => ({
    ...value,
    address: normalizeWledAddress(value.address),
  }),
  summary: ({ address, leds }) => `${address}, ${ledTotal(leds)} LEDs`,
  lighting: (saved) => {
    const href = wledUrl(saved.address);
    return {
      effects: 'WLED effects',
      ...(href ? { link: { label: 'Open WLED', href } } : {}),
    };
  },
};
