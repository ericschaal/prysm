import { useState } from 'react';
import type { Config } from '../service';
import { edges } from '../processingLocation';
import ChoiceButton from './ChoiceButton';

type Output = NonNullable<Config['embeddedOutput']>;
type Edge = (typeof edges)[number];
const labels: Record<Edge, string> = {
  top: 'Top',
  right: 'Right',
  bottom: 'Bottom',
  left: 'Left',
};

function wledUrl(text: string) {
  const input = text.trim();
  if (!input) return null;
  try {
    const url = new URL(input.includes('://') ? input : 'http://' + input);
    return (url.protocol === 'http:' || url.protocol === 'https:') &&
      url.href === url.origin + '/'
      ? url.href
      : null;
  } catch {
    return null;
  }
}

export default function WledSettings({
  saved,
  apply,
}: {
  saved: Config['embeddedOutput'];
  apply: (output: Output) => void;
}) {
  const [address, setAddress] = useState(saved?.url ?? '');
  const [counts, setCounts] = useState<Record<Edge, string>>(() => ({
    top: String(saved?.leds.top ?? 0),
    right: String(saved?.leds.right ?? 0),
    bottom: String(saved?.leds.bottom ?? 0),
    left: String(saved?.leds.left ?? 0),
  }));
  const [error, setError] = useState('');

  function submit() {
    const url = wledUrl(address);
    if (!url) {
      setError('Enter the WLED IP address or HTTP(S) base URL.');
      return;
    }
    if (edges.some((edge) => !/^\d+$/.test(counts[edge]))) {
      setError('Each edge needs a nonnegative whole number.');
      return;
    }
    const leds = {
      top: Number(counts.top),
      right: Number(counts.right),
      bottom: Number(counts.bottom),
      left: Number(counts.left),
    };
    const total = edges.reduce((sum, edge) => sum + leds[edge], 0);
    if (total < 1 || total > 65535) {
      setError(
        'Use between 1 and 65535 LEDs in total. Enter 0 for an unused edge.',
      );
      return;
    }
    apply({ type: 'wled', url, leds });
  }

  return (
    <>
      <label htmlFor="wled-url">WLED address</label>
      <input
        id="wled-url"
        type="text"
        value={address}
        onChange={(e) => setAddress(e.target.value)}
        spellCheck={false}
        autoComplete="off"
      />
      <div className="led-edges">
        {edges.map((edge) => (
          <label key={edge} htmlFor={'led-' + edge}>
            {labels[edge]} LEDs
            <input
              id={'led-' + edge}
              type="text"
              inputMode="numeric"
              value={counts[edge]}
              onChange={(e) => setCounts({ ...counts, [edge]: e.target.value })}
            />
          </label>
        ))}
      </div>
      {error && (
        <p role="alert" className="notice error">
          {error}
        </p>
      )}
      <ChoiceButton id="wled-apply" title="Use this setup" action={submit} />
    </>
  );
}
