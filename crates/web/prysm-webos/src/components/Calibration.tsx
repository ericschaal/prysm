import { useCallback, useEffect, useRef, useState } from 'react';
import PairingScreen from './PairingScreen';
import type { components } from '../../../prysm-web/control-ui/api.generated';
type View = components['schemas']['CalibrationView'];
const colors = ['red', 'green', 'blue', 'cyan', 'magenta', 'yellow'];
const shades = ['black', 'gray-32', 'gray-64', 'gray-128', 'gray-192', 'white'];

function patternFor(view: View | null) {
  switch (view?.step) {
    case 'white_balance':
      return 'gray-192';
    case 'grays':
      return shades[view.shade];
    case 'intensity':
      return ['clay', 'leaf', 'denim'][view.shade];
    case 'mixed':
      return ['coast', 'garden', 'dusk'][view.shade];
    case 'colors':
      return view.paused ? colors[view.shade] : 'colors';
    case 'review':
      return 'colors';
    default:
      return 'response';
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
  const videoRef = useRef<HTMLVideoElement>(null);
  const playingRef = useRef(false);
  const endedRef = useRef(false);
  const connected = !!view?.connected && (!view.ended || view.saved);
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
    let timeout: number;
    const relayWindow = relayRef.current?.contentWindow;
    function send(action: string) {
      relayWindow?.postMessage({ action, playing: playingRef.current }, origin);
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
      if (event.data.type === 'calibration-ready') send('begin');
      if (event.data.type === 'calibration-error' && 'message' in event.data)
        setError(String(event.data.message));
      if (event.data.type === 'calibration' && 'view' in event.data) {
        const next = event.data.view as View;
        if (
          !next ||
          typeof next.token !== 'string' ||
          typeof next.connected !== 'boolean' ||
          !shades[next.shade] ||
          !patternFor(next)
        )
          return;
        clearTimeout(timeout);
        timeout = window.setTimeout(() => {
          endedRef.current = true;
          setError('Receiver connection lost. Calibration is ending.');
          setView(null);
          send('cancel');
        }, 15000);
        setView(next);
        setError('');
        if (next.ended) {
          endedRef.current = true;
          clearTimeout(timeout);
        }
      }
    };
    const timer = window.setInterval(() => {
      if (!endedRef.current) send('poll');
    }, 1000);
    timeout = window.setTimeout(
      () =>
        setError(
          'Cannot reach Prysm. Check its web address and allowed hosts.',
        ),
      15000,
    );
    const leave = () => {
      endedRef.current = true;
      send('cancel');
      videoRef.current?.pause();
      videoRef.current?.removeAttribute('src');
      videoRef.current?.load();
    };
    const hidden = () => {
      if (document.hidden) {
        leave();
        setView(null);
        setError('Calibration ended because the TV app was hidden.');
      }
    };
    window.addEventListener('message', receive);
    window.addEventListener('pagehide', leave);
    document.addEventListener('visibilitychange', hidden);
    return () => {
      clearTimeout(timeout);
      clearInterval(timer);
      leave();
      window.removeEventListener('message', receive);
      window.removeEventListener('pagehide', leave);
      document.removeEventListener('visibilitychange', hidden);
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
  useEffect(() => {
    const element = videoRef.current;
    if (!connected || !element) return;
    playingRef.current = false;
    let current = true;
    const result = element.play();
    result?.catch((failure: unknown) => {
      if (
        current &&
        !(failure instanceof DOMException && failure.name === 'AbortError')
      ) {
        playbackFailed('Video could not play. Exit on the TV and try again.');
      }
    });
    return () => {
      current = false;
      playingRef.current = false;
      element.pause();
      element.removeAttribute('src');
      element.load();
    };
  }, [connected, pattern, playbackFailed]);
  return (
    <div className="calibration-screen">
      {origin ? (
        <iframe
          ref={relayRef}
          src={`${origin}/calibration-tv.html`}
          onLoad={() =>
            relayRef.current?.contentWindow?.postMessage(
              { action: 'begin' },
              origin,
            )
          }
          title="Prysm calibration connection"
          hidden
        />
      ) : null}
      {connected ? (
        <video
          key={pattern}
          ref={videoRef}
          src={`calibration/${pattern}.mp4`}
          muted
          loop
          playsInline
          onPlaying={() => {
            playingRef.current = true;
          }}
          onError={(event) => {
            if (event.currentTarget.hasAttribute('src'))
              playbackFailed('Video failed. Exit and try again.');
          }}
          aria-label="Calibration pattern"
        />
      ) : (
        <PairingScreen
          receiver={receiver}
          origin={origin}
          url={url}
          ended={view?.ended}
          error={error}
          connect={setOrigin}
          back={back}
        />
      )}
    </div>
  );
}
