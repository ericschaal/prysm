import {
  type RefObject,
  useEffectEvent,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import { streamPatterns, supportsPatternStream } from '../patternStream';

export interface PatternPlayback {
  readonly playing: boolean;
  release(): void;
}

export default function PatternVideo({
  pattern,
  ref,
  failed,
}: {
  pattern: string;
  ref: RefObject<PatternPlayback | null>;
  failed: (message: string) => void;
}) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [stream, setStream] = useState(supportsPatternStream);
  const [shown, setShown] = useState('');
  const requested = useEffectEvent(() => pattern);
  const file = stream ? null : pattern;
  useLayoutEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    let stop: (() => void) | undefined;
    function error() {
      if (ref.current !== playback) return;
      if (file) failed('Video failed. Exit and try again.');
      else {
        setShown('');
        setStream(false);
      }
    }
    const playback = {
      get playing() {
        return !video.paused && video.currentTime > 0;
      },
      release() {
        if (ref.current !== playback) return;
        // Reset the handle before load() can deliver events from the old source.
        ref.current = null;
        setShown('');
        stop?.();
        video.pause();
        video.removeAttribute('src');
        video.load();
      },
    };
    ref.current = playback;
    video.addEventListener('error', error);
    if (file) video.src = 'calibration/' + file + '.mp4';
    else {
      try {
        stop = streamPatterns(video, requested, setShown, error);
      } catch {
        error();
      }
    }
    video.play()?.catch((failure: unknown) => {
      if (!(failure instanceof DOMException && failure.name === 'AbortError'))
        error();
    });
    return () => {
      video.removeEventListener('error', error);
      playback.release();
    };
  }, [file, failed, ref]);
  return (
    <>
      <video
        ref={videoRef}
        onPlaying={() => {
          if (file) setShown(file);
        }}
        muted
        loop={!stream}
        playsInline
        aria-label="Calibration pattern"
      />
      {pattern !== shown && (
        <div key={pattern} className="pattern-pending" role="status">
          <span aria-hidden="true" />
          Changing color…
        </div>
      )}
    </>
  );
}
