import {
  type KeyboardEvent,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import qrcode from 'qrcode-generator';
import { arrowDirection } from '../hooks/useRemoteNavigation';
import Header from './Header';

function receiverAddress(receiver: string) {
  try {
    return `http://${new URL('http://' + receiver).hostname}:8787`;
  } catch {
    return '';
  }
}

type PairingProps = {
  receiver: string;
  origin: string;
  url: string;
  choosing: boolean;
  ended?: string | null;
  saved: boolean;
  error: string;
  connect: (origin: string) => void;
  back: () => void;
};

function pairingPhase({
  choosing,
  ended,
  saved,
  origin,
  url,
  error,
}: PairingProps) {
  if (saved) return 'saved';
  if (choosing) return 'standby';
  if (ended) return 'ended';
  if (!origin) return 'entry';
  if (url) return 'scan';
  if (error) return 'interrupted';
  return 'connecting';
}

const instructions = {
  entry: {
    kicker: '01  /  Receiver',
    title: 'Connect to Prysm.',
    lead: 'Use the address you open to control your lights. Your phone will guide you through the color adjustments.',
  },
  connecting: {
    kicker: '02  /  Phone',
    title: 'Getting your code.',
    lead: 'The TV is contacting your receiver for a pairing code.',
  },
  scan: {
    kicker: '02  /  Phone',
    title: 'Scan with your phone.',
    lead: 'Open your phone’s camera and point it at the code. Keep Prysm open on this TV.',
  },
  interrupted: {
    kicker: '02  /  Phone',
    title: 'Connection interrupted.',
    lead: 'Go back to check the receiver address and try again.',
  },
  standby: {
    kicker: 'Phone connected',
    title: 'Calibration in progress',
    lead: 'Choose a profile on your phone. The first colors will appear here when you reach the response check.',
  },
  ended: { kicker: 'Session complete', title: 'Calibration ended', lead: '' },
  saved: { kicker: 'Profile saved', title: 'Calibration finished', lead: '' },
};

export default function PairingScreen(props: PairingProps) {
  const { receiver, url, choosing, ended, error, connect, back } = props;
  const [edited, setEdited] = useState<string | null>(null);
  const [invalid, setInvalid] = useState('');
  const address = edited ?? receiverAddress(receiver);
  const phase = pairingPhase(props);
  const entry = phase === 'entry';
  const { kicker, title, lead } = instructions[phase];
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

  function submitAddress(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    try {
      const value = address.trim();
      const parsed = new URL(value.includes('://') ? value : 'http://' + value);
      if (
        (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') ||
        parsed.username ||
        parsed.password
      )
        throw Error();
      setInvalid('');
      connect(parsed.origin);
    } catch {
      setInvalid('Enter a receiver address, such as http://192.168.1.10:8787.');
      inputRef.current?.focus();
    }
  }

  function navigate(event: KeyboardEvent) {
    const direction = arrowDirection(event.keyCode);
    if (!direction || !entry) return;
    const active = document.activeElement;
    if (
      active === inputRef.current &&
      (direction === 'left' || direction === 'right')
    )
      return;
    event.preventDefault();
    if (active === backRef.current) {
      if (direction === 'up' || direction === 'right')
        connectRef.current?.focus();
    } else if (
      direction === 'left' ||
      (direction === 'down' && active === connectRef.current)
    ) {
      backRef.current?.focus();
    } else if (direction === 'up' && active === connectRef.current) {
      inputRef.current?.focus();
    } else if (direction === 'down' && active === inputRef.current) {
      connectRef.current?.focus();
    }
  }
  return (
    <main className="pairing-shell">
      <Header subtitle="Light calibration" />
      <div className={'pairing-layout' + (choosing ? ' pairing-standby' : '')}>
        <div className="pairing-copy" role={choosing ? 'status' : undefined}>
          <p className="pairing-kicker">{kicker}</p>
          <h2>{title}</h2>
          <p className="pairing-lead">{ended || lead}</p>
        </div>
        {!choosing && (
          <div
            className={
              'pairing-card' + (phase === 'scan' ? ' pairing-scan' : '')
            }
          >
            {entry && (
              <form noValidate onSubmit={submitAddress}>
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
            )}
            {phase === 'scan' && (
              <>
                <img src={qr} alt="Scan to open the calibration wizard" />
                <p className="pairing-status" role="status">
                  Waiting for your phone
                </p>
              </>
            )}
            {!entry && phase !== 'scan' && (
              <p className="pairing-status" role={error ? 'alert' : 'status'}>
                {error ||
                  (ended
                    ? 'Press Return on your remote to exit.'
                    : 'Connecting to receiver…')}
              </p>
            )}
            {phase === 'scan' && error && <p role="alert">{error}</p>}
          </div>
        )}
      </div>
      <footer className="pairing-footer">
        <button
          ref={backRef}
          onKeyDown={navigate}
          onMouseEnter={(e) => e.currentTarget.focus()}
          id="calibration-exit"
          onClick={back}
        >
          <span aria-hidden="true">←</span>{' '}
          {choosing || phase === 'saved' ? 'Exit calibration' : 'Back'}
        </button>
        {!ended && !choosing && (
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
