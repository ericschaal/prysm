import { useRef, useState } from 'react';
import type {
  LightingTarget,
  PreviewClient,
  Session,
} from '@prysm/application-client';
import { usePageVisible } from './live-colors';
import { usePreviewStream } from './preview-stream';

function idleMessage(target: LightingTarget, session: Session) {
  if (session.state === 'running') return 'Waiting for the first picture…';
  if (session.state === 'native')
    return 'The lights are showing their own effects, so there is no picture to show.';
  if (target === 'off')
    return 'Lights are off. Turn on Screen sync to see the picture and its colours.';
  return 'No picture yet. It appears while Screen sync is running.';
}

/**
 * The captured picture with the colours sent to each edge around it, for
 * checking black-bar removal and the LED colours. The stream is open only
 * while the panel is mounted, the page is visible and the portal is
 * connected: the portal counts viewers and encodes previews only while
 * somebody watches.
 */
export function PreviewPanel({
  client,
  online,
  target,
  session,
}: {
  client: PreviewClient;
  online: boolean;
  target: LightingTarget;
  session: Session;
}) {
  const [attempt, setAttempt] = useState(0);
  const visible = usePageVisible();
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const active = visible && online;
  const view = usePreviewStream(client, active, attempt, canvasRef);
  const retry = (
    <button type="button" onClick={() => setAttempt((count) => count + 1)}>
      Try again
    </button>
  );
  let message = null;
  if (!online)
    message = <p className="preview-message">Waiting for the connection…</p>;
  else if (view.kind === 'connecting')
    message = <p className="preview-message">Connecting to the preview…</p>;
  else if (view.kind === 'idle')
    message = <p className="preview-message">{idleMessage(target, session)}</p>;
  else if (view.kind === 'unavailable')
    message = (
      <p className="preview-message">
        This picture cannot be shown: {view.message}
      </p>
    );
  else if (view.kind === 'busy')
    message = (
      <div className="notice warning" role="alert">
        <p>
          Too many people are watching the preview. Close it in another window,
          then try again.
        </p>
        {retry}
      </div>
    );
  else if (view.kind === 'lost')
    message = (
      <div className="notice warning" role="alert">
        <p>The preview stopped{view.message ? `: ${view.message}` : '.'}</p>
        {retry}
      </div>
    );
  const showing = online && view.kind === 'frame';
  return (
    <div
      className="preview"
      data-view={online ? view.kind : 'offline'}
      aria-live="polite"
    >
      <canvas
        ref={canvasRef}
        className="preview-canvas"
        role="img"
        aria-label="Captured picture with the colours sent to the lights around it"
        hidden={!showing}
      />
      {showing ? (
        <p className="preview-info">
          {view.sampled
            ? `Picture ${view.source}. Black bars removed: colours come from the outlined ${view.sampled} area.`
            : `Picture ${view.source}.`}
        </p>
      ) : (
        message
      )}
    </div>
  );
}
