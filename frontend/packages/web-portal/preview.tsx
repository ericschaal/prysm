import { useRef, useState, type ReactNode } from 'react';
import type {
  PreviewClient,
  SessionReading,
  TargetReading,
} from '@prysm/application-client';
import type { LedEdges } from './adapters';
import { LedDrawing } from './led-drawing';
import { usePageVisible } from './live-colors';
import { drawPreview } from './preview-canvas';
import { usePreviewStream, type PreviewView } from './preview-stream';

/** What Home's preview needs from the portal. */
export interface PreviewSource {
  client: PreviewClient;
  online: boolean;
  target: TargetReading;
  session: SessionReading;
}

function idleMessage(target: TargetReading, session: SessionReading) {
  if (session.state === 'screen_sync') return 'Waiting for the first picture…';
  if (session.state === 'effects')
    return 'The lights are showing their own effects, so there is no picture to show.';
  if (target === 'off')
    return 'Lights are off. Turn on Screen sync to see the picture and its colours.';
  return 'No picture yet. It appears while Screen sync is running.';
}

/** The preview's state in one line, with a retry where one helps. */
function stateLine(
  view: PreviewView,
  source: PreviewSource,
  retry: ReactNode,
): ReactNode {
  if (!source.online) return 'Waiting for the connection…';
  switch (view.kind) {
    case 'connecting':
      return 'Connecting to the preview…';
    case 'idle':
      return idleMessage(source.target, source.session);
    case 'frame':
      return view.sampled
        ? `Picture ${view.source}. Black bars removed: colours come from the outlined ${view.sampled} area.`
        : `Picture ${view.source}.`;
    case 'unavailable':
      return `This picture can’t be shown: ${view.message}`;
    case 'busy':
      return (
        <>
          Too many people are watching the preview. Close it in another window.{' '}
          {retry}
        </>
      );
    case 'lost':
      return (
        <>
          The preview stopped{view.message ? `: ${view.message}` : '.'} {retry}
        </>
      );
  }
}

function PlayIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M8 5.5v13a1 1 0 0 0 1.5.86l11-6.5a1 1 0 0 0 0-1.72l-11-6.5A1 1 0 0 0 8 5.5Z" />
    </svg>
  );
}

function PauseIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <rect x="6" y="5" width="4" height="14" rx="1" />
      <rect x="14" y="5" width="4" height="14" rx="1" />
    </svg>
  );
}

/**
 * Home's TV drawing with the live preview in its screen: paused on every
 * visit, a play button on the screen; playing, the captured picture (black
 * bars dimmed, the sampled area outlined) with a pause button in its corner,
 * and the preview's state in one line under the drawing. The LED bands show
 * the colours as they always do. The stream is open only while playing, the
 * page is visible and the portal is connected: the portal counts viewers
 * and encodes previews only while somebody watches.
 */
export function PreviewDrawing({
  edges,
  caption,
  source,
}: {
  edges: LedEdges;
  caption: string;
  source: PreviewSource;
}) {
  const [playing, setPlaying] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const visible = usePageVisible();
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const toggleRef = useRef<HTMLButtonElement | null>(null);
  const view = usePreviewStream(
    source.client,
    playing && visible && source.online,
    attempt,
    canvasRef,
    drawPreview,
  );
  const retry = (
    <button
      type="button"
      className="link"
      onClick={() => {
        // The line changes under it: keep the focus on the preview.
        toggleRef.current?.focus();
        setAttempt((count) => count + 1);
      }}
    >
      Try again
    </button>
  );
  const showing = playing && source.online && view.kind === 'frame';
  const live = source.online ? view.kind : 'offline';
  const state = playing ? live : 'paused';
  return (
    <LedDrawing
      edges={edges}
      caption={caption}
      screen={
        <>
          <canvas
            ref={canvasRef}
            role="img"
            aria-label="Captured picture"
            hidden={!showing}
          />
          <button
            ref={toggleRef}
            type="button"
            className="led-map-play"
            data-playing={playing}
            aria-label={playing ? 'Pause preview' : 'Play preview'}
            onClick={() => setPlaying((now) => !now)}
          >
            {playing ? <PauseIcon /> : <PlayIcon />}
          </button>
        </>
      }
      note={
        <p className="led-map-note" data-view={state} aria-live="polite">
          {playing ? stateLine(view, source, retry) : null}
        </p>
      }
    />
  );
}
