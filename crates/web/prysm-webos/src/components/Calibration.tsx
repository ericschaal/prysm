import { useCallback, useEffect, useRef, useState } from 'react';
import patterns from '../../../calibration-patterns.json';
import PatternVideo, { type PatternPlayback } from './PatternVideo';
import PairingScreen from './PairingScreen';
import type { components } from '../../../prysm-web/control-ui/api.generated';
type View = components['schemas']['CalibrationView'];
const { colors, intensity, pairs } = patterns;

function patternFor(view: View | null) {
  switch (view?.step) {
    case 'choose':
      return view.startup_pending ? 'response' : null;
    case 'white_balance':
      return 'gray-192';
    case 'intensity':
      return intensity[view.shade]?.clip;
    case 'mixed':
      return pairs[view.shade]?.clip;
    case 'colors':
      if (!view.paused) return 'colors';
      return colors[view.shade]?.clip === 'blue'
        ? 'blue-pair'
        : colors[view.shade]?.clip;
    case 'review':
      return 'review-grid';
    case 'response':
      return 'response';
    default:
      return null;
  }
}

export default function Calibration({
  receiver,
  close,
}: {
  receiver: string;
  close: () => void;
}) {
  const [origin, setOrigin] = useState('');
  const [view, setView] = useState<View | null>(null);
  const [error, setError] = useState('');
  const relayRef = useRef<HTMLIFrameElement>(null);
  const playbackRef = useRef<PatternPlayback>(null);
  const endedRef = useRef(false);
  const connected = !!view?.connected && !view.ended;
  const pattern = patternFor(view);
  const url = view?.pairing ? `${origin}/#/calibrate/${view.pairing}` : '';
  const playbackFailed = useCallback(
    (message: string) => {
      endedRef.current = true;
      relayRef.current?.contentWindow?.postMessage(
        { action: 'cancel' },
        origin,
      );
      setView(null);
      setError(message);
    },
    [origin],
  );

  useEffect(() => {
    if (!origin) return;
    endedRef.current = false;
    let deadline = window.setTimeout(
      () =>
        setError(
          (reason) =>
            reason ||
            'Cannot reach Prysm. Check its web address and allowed hosts.',
        ),
      15000,
    );
    const relayWindow = relayRef.current?.contentWindow;
    function send(action: string) {
      relayWindow?.postMessage(
        { action, playing: playbackRef.current?.playing ?? false },
        origin,
      );
    }
    const receive = (event: MessageEvent<unknown>) => {
      if (endedRef.current) return;
      if (
        event.origin !== origin ||
        event.source !== relayWindow ||
        !event.data ||
        typeof event.data !== 'object' ||
        !('type' in event.data)
      )
        return;
      if (event.data.type === 'calibration-error' && 'message' in event.data) {
        if ('terminal' in event.data && event.data.terminal === true) {
          endedRef.current = true;
          clearTimeout(deadline);
          setView(null);
        }
        setError(String(event.data.message));
      }
      if (event.data.type === 'calibration' && 'view' in event.data) {
        const next = event.data.view as View;
        if (
          !next ||
          typeof next.token !== 'string' ||
          typeof next.connected !== 'boolean' ||
          !Number.isInteger(next.shade) ||
          next.shade < 0 ||
          next.shade >= colors.length ||
          (next.step !== 'choose' && !patternFor(next))
        )
          return;
        clearTimeout(deadline);
        setView(next);
        setError('');
        endedRef.current = !!next.ended;
        if (!next.ended)
          deadline = window.setTimeout(() => {
            if (endedRef.current) return;
            setError('Receiver connection lost. Reconnecting…');
            setView(null);
          }, 30000);
      }
    };
    const timer = window.setInterval(() => {
      if (!endedRef.current) send('poll');
    }, 1000);
    const leave = () => {
      endedRef.current = true;
      clearTimeout(deadline);
      send('cancel');
    };
    const hide = () => {
      if (endedRef.current) return;
      leave();
      playbackRef.current?.release();
      setView(null);
      setError('Calibration ended because the TV app was hidden.');
    };
    const visibility = () => {
      if (document.hidden) hide();
    };
    window.addEventListener('message', receive);
    window.addEventListener('pagehide', hide);
    document.addEventListener('visibilitychange', visibility);
    return () => {
      clearInterval(timer);
      leave();
      window.removeEventListener('message', receive);
      window.removeEventListener('pagehide', hide);
      document.removeEventListener('visibilitychange', visibility);
    };
  }, [origin]);
  const back = useCallback(() => {
    if (origin && !connected && !view?.ended) {
      setOrigin('');
      setView(null);
      setError('');
    } else {
      close();
    }
  }, [origin, connected, view?.ended, close]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.keyCode === 461 || event.keyCode === 27) {
        event.preventDefault();
        back();
      }
    };
    document.addEventListener('keydown', key);
    return () => document.removeEventListener('keydown', key);
  }, [back]);
  return (
    <div className="calibration-screen">
      {origin ? (
        <iframe
          ref={relayRef}
          src={`${origin}/calibration-tv.html`}
          title="Prysm calibration connection"
          hidden
        />
      ) : null}
      {connected && pattern ? (
        <PatternVideo
          pattern={pattern}
          ref={playbackRef}
          failed={playbackFailed}
        />
      ) : (
        <PairingScreen
          receiver={receiver}
          origin={origin}
          url={url}
          choosing={connected && view?.step === 'choose'}
          ended={view?.ended}
          saved={view?.saved === true}
          error={error}
          connect={setOrigin}
          back={back}
        />
      )}
    </div>
  );
}
