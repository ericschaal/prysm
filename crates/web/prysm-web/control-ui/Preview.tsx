import { useEffect, useRef, useState } from 'react';
import type { StateResponse } from './api';

import type { components } from './api.generated';

type PreviewMessage = components['schemas']['PreviewMessage'];
type PreviewFrame = Extract<PreviewMessage, { type: 'frame' }>;
const edges = ['top', 'right', 'bottom', 'left'] as const;
function isFrame(value: PreviewFrame): boolean {
  const crop = value.viewport;
  const dimension = (n: number) => Number.isInteger(n) && n > 0;
  return (
    typeof value.jpeg === 'string' &&
    value.jpeg.length <= 2_000_000 &&
    [value.width, value.height, value.source_width, value.source_height].every(
      dimension,
    ) &&
    value.width <= 480 &&
    value.height <= 480 &&
    value.source_width <= 4294967295 &&
    value.source_height <= 4294967295 &&
    (crop === null ||
      (typeof crop === 'object' &&
        Number.isInteger(crop.x) &&
        Number.isInteger(crop.y) &&
        crop.x >= 0 &&
        crop.y >= 0 &&
        dimension(crop.width) &&
        dimension(crop.height) &&
        crop.x + crop.width <= value.source_width &&
        crop.y + crop.height <= value.source_height)) &&
    edges.every(
      (edge) =>
        Array.isArray(value.colors?.[edge]) &&
        value.colors[edge].length > 0 &&
        value.colors[edge].length <= 480 &&
        value.colors[edge].every(
          (rgb) =>
            Array.isArray(rgb) &&
            rgb.length === 3 &&
            rgb.every((v) => Number.isInteger(v) && v >= 0 && v <= 255),
        ),
    )
  );
}
function draw(
  canvas: HTMLCanvasElement,
  image: HTMLImageElement,
  frame: PreviewFrame,
) {
  const context = canvas.getContext('2d');
  if (!context) throw Error('Canvas is unavailable in this browser.');
  const border = 12;
  canvas.width = frame.width + 2 * border;
  canvas.height = frame.height + 2 * border;
  context.fillStyle = '#10131c';
  context.fillRect(0, 0, canvas.width, canvas.height);
  context.drawImage(image, border, border, frame.width, frame.height);
  for (const edge of edges) {
    const colors = frame.colors[edge];
    colors.forEach((rgb, index) => {
      context.fillStyle = `rgb(${rgb.join(',')})`;
      const vertical = edge === 'left' || edge === 'right';
      const length = vertical ? frame.height : frame.width;
      const start = Math.floor((index * length) / colors.length);
      const end = Math.ceil(((index + 1) * length) / colors.length);
      if (vertical)
        context.fillRect(
          edge === 'left' ? 0 : border + frame.width,
          border + start,
          border,
          end - start,
        );
      else
        context.fillRect(
          border + start,
          edge === 'top' ? 0 : border + frame.height,
          end - start,
          border,
        );
    });
  }
  if (frame.viewport) {
    const crop = frame.viewport;
    context.strokeStyle = '#ffffff';
    context.lineWidth = 2;
    context.setLineDash([5, 4]);
    context.strokeRect(
      border + (crop.x * frame.width) / frame.source_width,
      border + (crop.y * frame.height) / frame.source_height,
      (crop.width * frame.width) / frame.source_width,
      (crop.height * frame.height) / frame.source_height,
    );
    context.setLineDash([]);
  }
}

function previewAvailabilityNote(data: StateResponse | null) {
  if (!data) return '';
  if (data.observed?.power === false)
    return 'Lights are off. Turn them on in Overview to use Screen sync.';
  if (data.source !== 'prysm')
    return 'WLED effects are selected. Choose Screen sync in Overview to follow your picture.';
  if (data.pipeline.phase === 'running' || data.pipeline.phase === 'starting')
    return '';
  return 'Screen sync is stopped. Any displayed picture is from an earlier capture.';
}

export function Preview({
  active,
  data,
}: {
  active: boolean;
  data: StateResponse | null;
}) {
  const [opened, setOpened] = useState(false);
  const [paused, setPaused] = useState(false);
  const [status, setStatus] = useState(
    'Open preview to inspect the current picture.',
  );
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const metadataRef = useRef<HTMLParagraphElement>(null);
  const [hasFrame, setHasFrame] = useState(false);
  const phase = data?.pipeline.phase ?? 'idle';
  const previewEnabled =
    data?.observed?.power === true &&
    data.source === 'prysm' &&
    phase === 'running';
  const previewNote = previewAvailabilityNote(data);
  if (!previewEnabled && (opened || paused)) {
    setOpened(false);
    setPaused(false);
  }
  useEffect(() => {
    if (!previewEnabled) return;
    if (!opened || paused || !active || document.hidden) return;
    let disposed = false;
    let outstanding = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    let image: HTMLImageElement | undefined;
    let lastRequest = 0;
    let localError = false;
    const url = new URL('/api/preview', window.location.href);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    const socket = new WebSocket(url);
    function stop(message: string) {
      if (disposed) return;
      localError = true;
      setStatus(message);
      setFailed(true);
      clearTimeout(timeout);
      clearTimeout(timer);
      socket.close();
    }
    function request() {
      if (
        disposed ||
        document.hidden ||
        outstanding ||
        socket.readyState !== WebSocket.OPEN
      )
        return;
      outstanding = true;
      lastRequest = performance.now();
      socket.send('next');
      timeout = setTimeout(
        () => stop('Preview timed out. Retry when the server is available.'),
        10000,
      );
    }
    function next() {
      outstanding = false;
      clearTimeout(timeout);
      if (!disposed && !document.hidden)
        timer = setTimeout(
          request,
          Math.max(0, 125 - (performance.now() - lastRequest)),
        );
    }
    socket.onopen = request;
    socket.onmessage = async (event) => {
      if (disposed || !outstanding || document.hidden) return;
      try {
        if (typeof event.data !== 'string' || event.data.length > 2_100_000)
          throw Error('Invalid preview response.');
        const value = JSON.parse(event.data) as PreviewMessage;
        if (value.type === 'frame') {
          if (!isFrame(value)) throw Error('Invalid preview frame.');
          image = new Image();
          image.src = `data:image/jpeg;base64,${value.jpeg}`;
          if (image.decode) await image.decode();
          else
            await new Promise<void>((resolve, reject) => {
              image!.onload = () => resolve();
              image!.onerror = () =>
                reject(Error('Could not decode the preview image.'));
            });
          if (disposed || document.hidden || localError) return;
          if (
            image.naturalWidth &&
            (image.naturalWidth !== value.width ||
              image.naturalHeight !== value.height)
          )
            throw Error('Preview dimensions do not match the image.');
          if (canvasRef.current) draw(canvasRef.current, image, value);
          setHasFrame(true);
          if (metadataRef.current)
            metadataRef.current.textContent = `Frame ${value.frame_id} · revision ${value.revision} · source ${value.source_width} × ${value.source_height}`;
          setStatus('Preview connected');
        } else if (value.type === 'idle')
          setStatus(
            'No processed picture yet. Opening preview does not start capture.',
          );
        else if (value.type === 'busy')
          setStatus('Preview encoder is busy. Retrying automatically.');
        else
          throw Error(
            value.type === 'error' && typeof value.message === 'string'
              ? value.message
              : 'Invalid preview response.',
          );
        next();
      } catch (error) {
        stop(error instanceof Error ? error.message : 'Preview failed.');
      }
    };
    socket.onerror = () =>
      stop('Preview connection failed. Lighting continues independently.');
    socket.onclose = () => {
      clearTimeout(timeout);
      clearTimeout(timer);
      if (!disposed && !localError) {
        setStatus('Preview disconnected. Retry to reconnect.');
        setFailed(true);
      }
    };
    const visibility = () => {
      if (document.hidden) {
        socket.close();
        clearTimeout(timer);
        clearTimeout(timeout);
      }
    };
    document.addEventListener('visibilitychange', visibility);
    return () => {
      disposed = true;
      setStatus('Connecting preview…');
      setFailed(false);
      clearTimeout(timer);
      clearTimeout(timeout);
      document.removeEventListener('visibilitychange', visibility);
      socket.close();
      if (image) image.src = '';
    };
  }, [opened, paused, active, attempt, previewEnabled]);
  useEffect(() => {
    const resume = () => {
      if (!document.hidden) setAttempt((previous) => previous + 1);
    };
    document.addEventListener('visibilitychange', resume);
    return () => document.removeEventListener('visibilitychange', resume);
  }, []);
  function badgeLabel() {
    if (!previewEnabled) return 'Unavailable';
    if (!opened) return 'Closed';
    if (paused) return 'Paused';
    if (failed) return 'Disconnected';
    return status === 'Preview connected' ? 'Live' : 'Waiting';
  }
  const badge = badgeLabel();
  return (
    <section className="preview card" aria-labelledby="preview-heading">
      <div className="section-heading">
        <div>
          <p className="eyebrow">SEE YOUR COLORS</p>
          <h2 id="preview-heading">Picture preview</h2>
        </div>
        <span
          role="status"
          aria-live="polite"
          className={`badge ${badge === 'Live' ? 'live-badge' : ''}`}
        >
          {badge}
        </span>
      </div>
      <p
        className={previewNote ? 'preview-note' : undefined}
        role={previewNote ? 'note' : undefined}
      >
        {previewNote || 'Your picture, with the colors sent to each edge.'}
      </p>
      <div className="preview-surface">
        <canvas
          ref={canvasRef}
          width="504"
          height="294"
          role="img"
          aria-label="Captured source image with detected crop and computed edge colors"
        />
        {!opened && !hasFrame ? <span>See what your lights see</span> : null}
      </div>
      <div className="preview-toolbar">
        <fieldset
          className="preview-controls"
          role="group"
          aria-label="Preview controls"
          disabled={!previewEnabled}
        >
          {!opened ? (
            <button
              type="button"
              aria-label="Open preview"
              onClick={() => {
                setOpened(true);
                setPaused(false);
              }}
            >
              <svg viewBox="0 0 20 20" aria-hidden="true">
                <path d="m7 4 9 6-9 6Z" />
              </svg>
              Open preview
            </button>
          ) : (
            <>
              <button
                type="button"
                aria-label={paused ? 'Resume preview' : 'Pause preview'}
                onClick={() => setPaused((previous) => !previous)}
              >
                <svg viewBox="0 0 20 20" aria-hidden="true">
                  {paused ? (
                    <path d="m7 4 9 6-9 6Z" />
                  ) : (
                    <path d="M5 4h4v12H5Zm7 0h4v12h-4Z" />
                  )}
                </svg>
                {paused ? 'Resume' : 'Pause'}
              </button>
              <button
                type="button"
                aria-label="Close preview"
                onClick={() => setOpened(false)}
              >
                <svg viewBox="0 0 20 20" aria-hidden="true">
                  <path d="M5 5h10v10H5Z" />
                </svg>
                Close
              </button>
              {failed ? (
                <button
                  type="button"
                  aria-label="Retry preview"
                  onClick={() => setAttempt((previous) => previous + 1)}
                >
                  Retry
                </button>
              ) : null}
            </>
          )}
        </fieldset>
      </div>
      {previewEnabled && opened && !paused && failed ? (
        <p className="preview-state error" role="alert">
          {status}
        </p>
      ) : null}
      <details className="diagnostics">
        <summary>Frame details</summary>
        <p className="preview-meta" ref={metadataRef}>
          No frame displayed
        </p>
      </details>
    </section>
  );
}
