import {
  type KeyboardEvent,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import qrcode from 'qrcode-generator';
import Header from './Header';

function receiverAddress(receiver: string) {
  try {
    return `http://${new URL('http://' + receiver).hostname}:8787`;
  } catch {
    return '';
  }
}

export default function PairingScreen({
  receiver,
  origin,
  url,
  ended,
  error,
  connect,
  back,
}: {
  receiver: string;
  origin: string;
  url: string;
  ended?: string | null;
  error: string;
  connect: (origin: string) => void;
  back: () => void;
}) {
  const [edited, setEdited] = useState<string | null>(null);
  const [invalid, setInvalid] = useState('');
  const address = edited ?? receiverAddress(receiver);
  const entry = !origin && !ended;
  const connectRef = useRef<HTMLButtonElement>(null);
  const backRef = useRef<HTMLButtonElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const qr = useMemo(() => {
    if (!url) return '';
    const code = qrcode(0, 'M');
    code.addData(url);
    code.make();
    return code.createDataURL(10, 40);
  }, [url]);
  useLayoutEffect(() => {
    (entry ? connectRef : backRef).current?.focus();
  }, [entry]);

  function navigate(event: KeyboardEvent) {
    const key = event.keyCode;
    if ([37, 38, 39, 40].indexOf(key) < 0 || !entry) return;
    const active = document.activeElement;
    if (active === inputRef.current && (key === 37 || key === 39)) return;
    event.preventDefault();
    if (active === backRef.current) {
      if (key === 38 || key === 39) connectRef.current?.focus();
    } else if (key === 37 || (key === 40 && active === connectRef.current)) {
      backRef.current?.focus();
    } else if (key === 38 && active === connectRef.current) {
      inputRef.current?.focus();
    } else if (key === 40 && active === inputRef.current) {
      connectRef.current?.focus();
    }
  }
  return (
    <main className="pairing-shell">
      <Header subtitle="Light calibration" />
      <div className="pairing-layout">
        <div className="pairing-copy">
          <p className="pairing-kicker">
            {ended
              ? 'Session complete'
              : entry
                ? '01  /  Receiver'
                : '02  /  Phone'}
          </p>
          <h2>
            {ended
              ? 'Calibration ended'
              : entry
                ? 'Connect to Prysm.'
                : url
                  ? 'Scan with your phone.'
                  : error
                    ? 'Connection interrupted.'
                    : 'Getting your code.'}
          </h2>
          <p className="pairing-lead">
            {ended ||
              (entry
                ? 'Use the address you open to control your lights. Your phone will guide you through the color adjustments.'
                : url
                  ? 'Open your phone’s camera and point it at the code. Keep Prysm open on this TV.'
                  : error
                    ? 'Go back to check the receiver address and try again.'
                    : 'The TV is contacting your receiver for a pairing code.')}
          </p>
        </div>
        <div className={'pairing-card' + (url ? ' pairing-scan' : '')}>
          {entry ? (
            <form
              noValidate
              onSubmit={(event) => {
                event.preventDefault();
                try {
                  const value = address.trim();
                  const parsed = new URL(
                    value.includes('://') ? value : 'http://' + value,
                  );
                  if (
                    ['http:', 'https:'].indexOf(parsed.protocol) < 0 ||
                    parsed.username ||
                    parsed.password
                  )
                    throw Error();
                  setInvalid('');
                  connect(parsed.origin);
                } catch {
                  setInvalid(
                    'Enter a receiver address, such as http://192.168.1.10:8787.',
                  );
                  inputRef.current?.focus();
                }
              }}
            >
              <label htmlFor="calibration-address">Prysm web address</label>
              <input
                ref={inputRef}
                onKeyDown={navigate}
                type="url"
                id="calibration-address"
                autoComplete="off"
                spellCheck={false}
                placeholder="http://192.168.1.10:8787"
                value={address}
                aria-invalid={!!invalid}
                aria-describedby={invalid ? 'pairing-error' : undefined}
                onChange={(event) => {
                  setEdited(event.target.value);
                  setInvalid('');
                }}
              />
              {invalid && (
                <p id="pairing-error" role="alert">
                  {invalid}
                </p>
              )}
              <button
                ref={connectRef}
                onKeyDown={navigate}
                onMouseEnter={(e) => e.currentTarget.focus()}
                className="pairing-primary"
                id="calibration-connect"
                type="submit"
              >
                Show QR code <span aria-hidden="true">→</span>
              </button>
            </form>
          ) : url && !ended ? (
            <>
              <img src={qr} alt="Scan to open the calibration wizard" />
              <p className="pairing-status" role="status">
                Waiting for your phone
              </p>
            </>
          ) : (
            <p className="pairing-status" role={error ? 'alert' : 'status'}>
              {error || (ended ? 'Session closed' : 'Connecting to receiver…')}
            </p>
          )}
          {url && error && <p role="alert">{error}</p>}
        </div>
      </div>
      <footer className="pairing-footer">
        <button
          ref={backRef}
          onKeyDown={navigate}
          onMouseEnter={(e) => e.currentTarget.focus()}
          id="calibration-exit"
          onClick={back}
        >
          <span aria-hidden="true">←</span> Back
        </button>
        {!ended && (
          <p>
            {entry
              ? 'Next: pair your phone'
              : 'Use the same network as your TV'}
          </p>
        )}
      </footer>
    </main>
  );
}
