import { useEffect, useId, useRef, useState, type RefObject } from 'react';
import type {
  LightingTarget,
  PreviewClient,
  PreviewFrame,
  PreviewStream,
  Session,
} from '@prysm/application-client';
import { failureOf, same } from './adapters';

/** Canvas pixels around the picture: a colour strip, then a gap. */
const STRIP = 12;
const GAP = 4;
const BORDER = STRIP + GAP;

interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}
type Rgb = readonly [number, number, number];

/**
 * Where everything goes on the canvas: the picture inside a border, each
 * edge's colours spread evenly along it in screen orientation (top and bottom
 * left to right, left and right top to bottom, as the stream sends them), and
 * the sampled area when black bars were removed.
 */
export function previewLayout(frame: PreviewFrame) {
  const { width, height } = frame;
  const image = { x: BORDER, y: BORDER, width, height };
  const along = (
    colors: readonly Rgb[],
    length: number,
    place: (start: number, size: number) => Rect,
  ) =>
    colors.map((color, index) => {
      // Whole pixels that meet exactly, so no seams show between colours.
      const start = Math.floor((index * length) / colors.length);
      const end = Math.floor(((index + 1) * length) / colors.length);
      return { ...place(BORDER + start, end - start), color };
    });
  const strips = [
    ...along(frame.colors.top, width, (x, size) => ({ x, y: 0, width: size, height: STRIP })),
    ...along(frame.colors.bottom, width, (x, size) => ({
      x,
      y: BORDER + height + GAP,
      width: size,
      height: STRIP,
    })),
    ...along(frame.colors.left, height, (y, size) => ({ x: 0, y, width: STRIP, height: size })),
    ...along(frame.colors.right, height, (y, size) => ({
      x: BORDER + width + GAP,
      y,
      width: STRIP,
      height: size,
    })),
  ];
  const { viewport, source_width, source_height } = frame;
  const whole =
    viewport.x === 0 &&
    viewport.y === 0 &&
    viewport.width === source_width &&
    viewport.height === source_height;
  const sx = source_width ? width / source_width : 0;
  const sy = source_height ? height / source_height : 0;
  return {
    width: width + 2 * BORDER,
    height: height + 2 * BORDER,
    image,
    strips,
    /** The sampled area on the canvas, or `null` when it is the whole frame. */
    viewport: whole
      ? null
      : {
          x: BORDER + viewport.x * sx,
          y: BORDER + viewport.y * sy,
          width: viewport.width * sx,
          height: viewport.height * sy,
        },
  };
}

/**
 * Draw one preview. `image` is the decoded JPEG, or `null` where the browser
 * cannot decode it (the colours are still drawn).
 */
export function drawPreview(
  canvas: HTMLCanvasElement,
  image: CanvasImageSource | null,
  frame: PreviewFrame,
) {
  const layout = previewLayout(frame);
  // Resizing clears the canvas, so only do it when the size changes.
  if (canvas.width !== layout.width) canvas.width = layout.width;
  if (canvas.height !== layout.height) canvas.height = layout.height;
  const context = canvas.getContext('2d');
  if (!context) return;
  context.clearRect(0, 0, layout.width, layout.height);
  for (const { x, y, width, height, color } of layout.strips) {
    context.fillStyle = `rgb(${color[0]}, ${color[1]}, ${color[2]})`;
    context.fillRect(x, y, width, height);
  }
  const { x, y, width, height } = layout.image;
  context.fillStyle = '#15161f';
  context.fillRect(x, y, width, height);
  if (image) context.drawImage(image, x, y, width, height);
  const viewport = layout.viewport;
  if (!viewport) return;
  // Dim what was cut off and outline what was sampled.
  context.fillStyle = 'rgba(0, 0, 0, 0.55)';
  context.beginPath();
  context.rect(x, y, width, height);
  context.rect(viewport.x, viewport.y, viewport.width, viewport.height);
  context.fill('evenodd');
  context.setLineDash([6, 4]);
  context.lineWidth = 2;
  context.strokeStyle = '#ffffff';
  context.strokeRect(viewport.x + 1, viewport.y + 1, viewport.width - 2, viewport.height - 2);
  context.setLineDash([]);
}

/** The JPEG as an image the canvas can draw; `null` if the browser cannot. */
async function decode(frame: PreviewFrame) {
  // The portal's CSP blocks data: and blob: images, but not canvas drawing.
  const create = window.createImageBitmap as typeof createImageBitmap | undefined;
  if (typeof create !== 'function') return null;
  const binary = atob(frame.jpeg);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++)
    bytes[index] = binary.charCodeAt(index);
  return create(new Blob([bytes], { type: 'image/jpeg' }));
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

function usePageVisible() {
  const [visible, setVisible] = useState(
    () => document.visibilityState !== 'hidden',
  );
  useEffect(() => {
    const changed = () => setVisible(document.visibilityState !== 'hidden');
    document.addEventListener('visibilitychange', changed);
    return () => document.removeEventListener('visibilitychange', changed);
  }, []);
  return visible;
}

/**
 * Stream previews into `canvas` while `active`. Decoding takes the newest
 * frame and skips any that arrived meanwhile, so a slow device shows fewer
 * frames rather than falling behind.
 */
function usePreviewStream(
  client: PreviewClient,
  active: boolean,
  attempt: number,
  canvas: RefObject<HTMLCanvasElement | null>,
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
            if (!abort.signal.aborted && canvas.current)
              drawPreview(canvas.current, image, frame);
          } finally {
            image?.close();
          }
        } catch {
          // A frame that cannot be decoded is skipped; the next one may work.
        }
      }
      painting = false;
    }
    show({ kind: 'connecting' });
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
    };
  }, [client, active, attempt, canvas]);
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
 * The captured picture with the colours sent to each edge around it. The
 * stream is open only while the panel is shown, the page is visible and the
 * portal is connected: the portal counts viewers and encodes previews only
 * while somebody watches.
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
  const [open, setOpen] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const visible = usePageVisible();
  const canvas = useRef<HTMLCanvasElement | null>(null);
  const headingId = useId();
  const bodyId = useId();
  const active = open && visible && online;
  const view = usePreviewStream(client, active, attempt, canvas);
  const retry = (
    <button type="button" onClick={() => setAttempt((count) => count + 1)}>
      Try again
    </button>
  );
  let message = null;
  if (!online) message = <p className="preview-message">Waiting for the connection…</p>;
  else if (view.kind === 'connecting')
    message = <p className="preview-message">Connecting to the preview…</p>;
  else if (view.kind === 'idle')
    message = <p className="preview-message">{idleMessage(target, session)}</p>;
  else if (view.kind === 'unavailable')
    message = (
      <p className="preview-message">This picture cannot be shown: {view.message}</p>
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
    <section className="card preview" aria-labelledby={headingId}>
      <div className="card-heading">
        <h2 id={headingId}>Preview</h2>
        <button
          type="button"
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={() => setOpen(!open)}
        >
          {open ? 'Hide preview' : 'Show preview'}
        </button>
      </div>
      <p className="intro">
        The captured picture, with the colours sent to each edge of your lights
        around it.
      </p>
      {open ? (
        <div id={bodyId} className="preview-body" aria-live="polite">
          <canvas
            ref={canvas}
            className="preview-canvas"
            role="img"
            aria-label="Captured picture with the colours sent to the lights around it"
            hidden={!showing}
          />
          {showing ? (
            <p className="help preview-info">
              {view.sampled
                ? `Picture ${view.source}. Black bars removed: colours come from the outlined ${view.sampled} area.`
                : `Picture ${view.source}.`}
            </p>
          ) : (
            message
          )}
        </div>
      ) : null}
    </section>
  );
}
