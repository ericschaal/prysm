import { useLayoutEffect, useMemo, useRef } from 'react';
import qrcode from 'qrcode-generator';
import Header from './Header';

type PairingProps = {
  url: string;
  choosing: boolean;
  ended?: string | null;
  saved: boolean;
  error: string;
  back: () => void;
};

function pairingPhase({ choosing, ended, saved, url, error }: PairingProps) {
  if (saved) return 'saved';
  if (choosing) return 'standby';
  if (ended) return 'ended';
  if (url) return 'scan';
  if (error) return 'interrupted';
  return 'connecting';
}

const instructions = {
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
  const { url, choosing, ended, error, back } = props;
  const phase = pairingPhase(props);
  const { kicker, title, lead } = instructions[phase];
  const backRef = useRef<HTMLButtonElement>(null);
  const qr = useMemo(() => {
    if (!url) return '';
    const code = qrcode(0, 'M');
    code.addData(url);
    code.make();
    return code.createDataURL(10, 40);
  }, [url]);
  useLayoutEffect(() => {
    backRef.current?.focus();
  }, []);
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
            {phase === 'scan' && (
              <>
                <img src={qr} alt="Scan to open the calibration wizard" />
                <p className="pairing-status" role="status">
                  Waiting for your phone
                </p>
              </>
            )}
            {phase !== 'scan' && (
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
          onMouseEnter={(e) => e.currentTarget.focus()}
          id="calibration-exit"
          onClick={back}
        >
          <span aria-hidden="true">←</span>{' '}
          {choosing || phase === 'saved' ? 'Exit calibration' : 'Back'}
        </button>
        {!ended && !choosing && <p>Use the same network as your TV</p>}
      </footer>
    </main>
  );
}
