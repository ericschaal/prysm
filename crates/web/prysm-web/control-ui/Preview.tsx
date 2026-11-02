import { useEffect, useRef, useState } from 'react';
import type { StateResponse } from './api';

type RGB = [number, number, number];
type PreviewFrame = { type: 'frame'; frame_id: number | string; revision: string; source_width: number; source_height: number; width: number; height: number; viewport: null | { x: number; y: number; width: number; height: number }; colors: Record<'top' | 'right' | 'bottom' | 'left', RGB[]>; jpeg: string };
const edges = ['top', 'right', 'bottom', 'left'] as const;
function isFrame(value: PreviewFrame): boolean {
  const crop = value.viewport;
  const dimension = (n: number) => Number.isInteger(n) && n > 0;
  return typeof value.jpeg === 'string' && value.jpeg.length <= 2_000_000 &&
    [value.width, value.height, value.source_width, value.source_height].every(dimension) &&
    value.width <= 480 && value.height <= 480 && value.source_width <= 4294967295 && value.source_height <= 4294967295 &&
    (crop === null || (typeof crop === 'object' && Number.isInteger(crop.x) && Number.isInteger(crop.y) && crop.x >= 0 && crop.y >= 0 && dimension(crop.width) && dimension(crop.height) && crop.x + crop.width <= value.source_width && crop.y + crop.height <= value.source_height)) &&
    edges.every(edge => Array.isArray(value.colors?.[edge]) && value.colors[edge].length > 0 && value.colors[edge].length <= 480 && value.colors[edge].every(rgb => Array.isArray(rgb) && rgb.length === 3 && rgb.every(v => Number.isInteger(v) && v >= 0 && v <= 255)));
}
function draw(canvas: HTMLCanvasElement, image: HTMLImageElement, frame: PreviewFrame) {
  const context = canvas.getContext('2d');
  if (!context) throw Error('Canvas is unavailable in this browser.');
  const border = 12;
  canvas.width = frame.width + 2 * border; canvas.height = frame.height + 2 * border;
  context.fillStyle = '#10131c'; context.fillRect(0, 0, canvas.width, canvas.height);
  context.drawImage(image, border, border, frame.width, frame.height);
  for (const edge of edges) {
    const colors = frame.colors[edge];
    colors.forEach((rgb, index) => {
      context.fillStyle = `rgb(${rgb.join(',')})`;
      const vertical = edge === 'left' || edge === 'right';
      const length = vertical ? frame.height : frame.width;
      const start = Math.floor(index * length / colors.length);
      const end = Math.ceil((index + 1) * length / colors.length);
      if (vertical) context.fillRect(edge === 'left' ? 0 : border + frame.width, border + start, border, end - start);
      else context.fillRect(border + start, edge === 'top' ? 0 : border + frame.height, end - start, border);
    });
  }
  if (frame.viewport) {
    const crop = frame.viewport;
    context.strokeStyle = '#ffffff'; context.lineWidth = 2; context.setLineDash([5, 4]);
    context.strokeRect(border + crop.x * frame.width / frame.source_width, border + crop.y * frame.height / frame.source_height, crop.width * frame.width / frame.source_width, crop.height * frame.height / frame.source_height);
    context.setLineDash([]);
  }
}

export function Preview({ active, data }: { active: boolean; data: StateResponse | null }) {
  const [opened, setOpened] = useState(false);
  const [paused, setPaused] = useState(false);
  const [status, setStatus] = useState('Open preview to inspect the current picture.');
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const canvas = useRef<HTMLCanvasElement>(null);
  const metadata = useRef<HTMLParagraphElement>(null);
  const hasFrame = useRef(false);
  const phase = data?.pipeline.phase ?? 'idle';
  const stale = phase !== 'running';
  useEffect(() => {
    if (!opened || paused || !active || document.hidden) return;
    let disposed = false;
    let outstanding = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    let image: HTMLImageElement | undefined;
    let lastRequest = 0;
    let localError = false;
    const url = new URL('/api/preview', window.location.href); url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    const socket = new WebSocket(url);
    setStatus('Connecting preview…'); setFailed(false);
    function stop(message: string) { if (disposed) return; localError = true; setStatus(message); setFailed(true); clearTimeout(timeout); clearTimeout(timer); socket.close(); }
    function request() {
      if (disposed || document.hidden || outstanding || socket.readyState !== WebSocket.OPEN) return;
      outstanding = true; lastRequest = performance.now(); socket.send('next');
      timeout = setTimeout(() => stop('Preview timed out. Retry when the server is available.'), 10000);
    }
    function next() { outstanding = false; clearTimeout(timeout); if (!disposed && !document.hidden) timer = setTimeout(request, Math.max(0, 125 - (performance.now() - lastRequest))); }
    socket.onopen = request;
    socket.onmessage = async event => {
      if (disposed || !outstanding || document.hidden) return;
      try {
        if (typeof event.data !== 'string' || event.data.length > 2_100_000) throw Error('Invalid preview response.');
        const value = JSON.parse(event.data);
        if (value.type === 'frame') {
          if (!isFrame(value)) throw Error('Invalid preview frame.');
          image = new Image(); image.src = `data:image/jpeg;base64,${value.jpeg}`;
          if (image.decode) await image.decode(); else await new Promise<void>((resolve, reject) => { image!.onload = () => resolve(); image!.onerror = () => reject(Error('Could not decode the preview image.')); });
          if (disposed || document.hidden || localError) return;
          if (image.naturalWidth && (image.naturalWidth !== value.width || image.naturalHeight !== value.height)) throw Error('Preview dimensions do not match the image.');
          if (canvas.current) draw(canvas.current, image, value);
          hasFrame.current = true;
          if (metadata.current) metadata.current.textContent = `Frame ${value.frame_id} · revision ${value.revision} · source ${value.source_width} × ${value.source_height}`;
          setStatus('Preview connected');
        } else if (value.type === 'idle') setStatus('No processed picture yet. Opening preview does not start capture.');
        else if (value.type === 'busy') setStatus('Preview encoder is busy. Retrying automatically.');
        else throw Error(value.type === 'error' && typeof value.message === 'string' ? value.message : 'Invalid preview response.');
        next();
      } catch (error) { stop(error instanceof Error ? error.message : 'Preview failed.'); }
    };
    socket.onerror = () => stop('Preview connection failed. Lighting continues independently.');
    socket.onclose = () => { clearTimeout(timeout); clearTimeout(timer); if (!disposed && !localError) { setStatus('Preview disconnected. Retry to reconnect.'); setFailed(true); } };
    const visibility = () => { if (document.hidden) { socket.close(); clearTimeout(timer); clearTimeout(timeout); } };
    document.addEventListener('visibilitychange', visibility);
    return () => { disposed = true; clearTimeout(timer); clearTimeout(timeout); document.removeEventListener('visibilitychange', visibility); socket.close(); if (image) image.src = ''; };
  }, [opened, paused, active, attempt]);
  useEffect(() => { const resume = () => { if (!document.hidden) setAttempt(previous => previous + 1); }; document.addEventListener('visibilitychange', resume); return () => document.removeEventListener('visibilitychange', resume); }, []);
  return <section className="preview card" aria-labelledby="preview-heading">
    <div className="section-heading"><div><p className="eyebrow">PICTURE OBSERVER</p><h2 id="preview-heading">Preview</h2></div><span className="badge">{phase}</span></div>
    <p>The image, detected crop and perimeter colors come from the same processing pass.</p>
    <div className="preview-surface"><canvas ref={canvas} width="504" height="294" role="img" aria-label="Captured source image with detected crop and computed edge colors" />{!opened && !hasFrame.current ? <span>Preview is closed</span> : null}</div>
    <p className="preview-meta" ref={metadata}>No frame displayed</p>
    <p className="preview-state" role="status" aria-live="polite">{!opened ? 'Preview closed.' : paused ? 'Preview paused.' : status} {hasFrame.current && (stale || paused || !opened || failed) ? `Last displayed image retained · capture ${phase}.` : null}</p>
    <div className="button-row">{!opened ? <button type="button" className="primary" onClick={() => { setOpened(true); setPaused(false); }}>Open preview</button> : <><button type="button" onClick={() => setPaused(previous => !previous)}>{paused ? 'Resume preview' : 'Pause preview'}</button><button type="button" onClick={() => setOpened(false)}>Close preview</button>{failed ? <button type="button" onClick={() => setAttempt(previous => previous + 1)}>Retry preview</button> : null}</>}</div>
    <p className="caption">Diagnostic preview · up to 8 fps. Capture waiting or stopped? The last image is not live.</p>
  </section>;
}
