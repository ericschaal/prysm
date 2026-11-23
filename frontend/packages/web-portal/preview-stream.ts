import { useEffect, useState, type RefObject } from 'react';
import type {
  PreviewClient,
  PreviewFrame,
  PreviewStream,
} from '@prysm/application-client';
import { failureOf, same } from '@prysm/application-client';
import { sampledArea } from './preview-canvas';

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

export type PreviewView =
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

/** Draws one decoded preview on a canvas. */
type Draw = (
  canvas: HTMLCanvasElement,
  image: CanvasImageSource | null,
  frame: PreviewFrame,
) => void;

/**
 * Stream previews into `canvasRef` while `active`, drawn by `draw` (a stable
 * function). Decoding takes the newest frame and skips any that arrived
 * meanwhile, so a slow device shows fewer frames rather than falling behind.
 */
export function usePreviewStream(
  client: PreviewClient,
  active: boolean,
  attempt: number,
  canvasRef: RefObject<HTMLCanvasElement | null>,
  draw: Draw,
) {
  const [view, setView] = useState<PreviewView>({ kind: 'connecting' });
  useEffect(() => {
    if (!active) return;
    const abort = new AbortController();
    let stream: PreviewStream | undefined;
    let latest: PreviewFrame | null = null;
    let painting = false;
    const show = (next: PreviewView) => {
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
              draw(canvasRef.current, image, frame);
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
            show({
              kind: 'frame',
              source: size(source_width, source_height),
              sampled: sampledArea(event)
                ? size(viewport.width, viewport.height)
                : null,
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
  }, [client, active, attempt, canvasRef, draw]);
  return view;
}
