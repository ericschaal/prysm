import { useEffect, useRef, useState, type RefObject } from 'react';
import type {
  LightingTarget,
  PreviewClient,
  PreviewFrame,
  PreviewStream,
  Session,
} from '@prysm/application-client';
import { failureOf, same } from '@prysm/application-client';
import { usePageVisible } from './live-colors';
import { drawPreview } from './preview-canvas';

/** The JPEG as an image the canvas can draw; `null` if the browser cannot. */
async function decode(frame: PreviewFrame) {
  // The portal's CSP blocks data: and blob: images, but not canvas drawing.
  if (typeof window.createImageBitmap !== 'function') return null;
  const binary = atob(frame.jpeg);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++)
    bytes[index] = binary.charCodeAt(index);
  return window.createImageBitmap(new Blob([bytes], { type: 'image/jpeg' }));
}

type View =
  | { kind: 'connecting' }
  | { kind: 'idle' }
  | {
      kind: 'frame';
      source: string;
      /** The sampled size when black bars were removed. */
      sampled: string | null;
    }
  | { kind: 'unavailable'; message: string }
  | { kind: 'busy' }
  | { kind: 'lost'; message: string };

const size = (width: number, height: number) => `${width} × ${height}`;

/**
 * Stream previews into `canvasRef` while `active`. Decoding takes the newest
 * frame and skips any that arrived meanwhile, so a slow device shows fewer
 * frames rather than falling behind.
 */
function usePreviewStream(
  client: PreviewClient,
  active: boolean,
  attempt: number,
  canvasRef: RefObject<HTMLCanvasElement | null>,
) {
  const [view, setView] = useState<View>({ kind: 'connecting' });
  useEffect(() => {
    if (!active) return;
    const abort = new AbortController();
    let stream: PreviewStream | undefined;
    let latest: PreviewFrame | null = null;
    let painting = false;
    const show = (next: View) => {
      if (!abort.signal.aborted)
        setView((current) => (same(current, next) ? current : next));
    };
    async function paint() {
      if (painting) return;
      painting = true;
      while (latest && !abort.signal.aborted) {
        const frame: PreviewFrame = latest;
        latest = null;
        try {
          const image = await decode(frame);
          try {
            if (!abort.signal.aborted && canvasRef.current)
              drawPreview(canvasRef.current, image, frame);
          } finally {
            image?.close();
          }
        } catch {
          // A frame that cannot be decoded is skipped; the next one may work.
        }
      }
      painting = false;
    }
    void (async () => {
      try {
        stream = await client.preview(abort.signal);
        if (abort.signal.aborted) return stream.close();
        for await (const event of stream.events) {
          if (abort.signal.aborted) return;
          if (event.type === 'idle') {
            latest = null;
            show({ kind: 'idle' });
          } else if (event.type === 'unavailable') {
            latest = null;
            show({ kind: 'unavailable', message: event.message });
          } else {
            const { viewport, source_width, source_height } = event;
            const whole =
              viewport.x === 0 &&
              viewport.y === 0 &&
              viewport.width === source_width &&
              viewport.height === source_height;
            show({
              kind: 'frame',
              source: size(source_width, source_height),
              sampled: whole ? null : size(viewport.width, viewport.height),
            });
            latest = event;
            void paint();
          }
        }
        show({ kind: 'lost', message: '' });
      } catch (error) {
        const failure = failureOf(error);
        show(
          failure.kind === 'busy'
            ? { kind: 'busy' }
            : { kind: 'lost', message: failure.message },
        );
      }
    })();
    return () => {
      abort.abort();
      stream?.close();
      // The next stream starts from scratch.
      setView({ kind: 'connecting' });
    };
  }, [client, active, attempt, canvasRef]);
  return view;
}

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
