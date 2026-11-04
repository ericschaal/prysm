import patterns from '../../calibration-patterns.json';
import bundledFragments from './pattern-fragments.json';

const mime = 'video/mp4; codecs="avc1.42c01e"';
const cycles: Record<string, string[]> = {
  response: patterns.response,
  colors: patterns.colors.map(({ clip }) => clip),
};
const clips = new Map<string, Uint8Array<ArrayBuffer>>();
const fragments: Readonly<Record<string, string>> = bundledFragments;
const fragmentsPerPatch = 4; // Four half-second fragments hold a cycle color for two seconds.

function fragment(name: string) {
  let bytes = clips.get(name);
  if (!bytes) {
    const encoded = fragments[name];
    if (!encoded) throw Error(`Missing calibration fragment: ${name}`);
    bytes = Uint8Array.from(atob(encoded), (c) => c.charCodeAt(0));
    clips.set(name, bytes);
  }
  return bytes;
}

// Keep a single decoder clock running. Every fragment starts on a keyframe.
export function streamPatterns(
  video: HTMLVideoElement,
  requested: () => string,
  presented: (pattern: string) => void,
  fallback: () => void,
) {
  const source = new MediaSource();
  const url = URL.createObjectURL(source);
  let buffer: SourceBuffer | undefined;
  let closed = false;
  let lastPattern = '';
  let fragmentIndex = 0;
  let lastTime = -1;
  let lastProgress = performance.now();
  const changes: { start: number; pattern: string }[] = [];

  function pump() {
    if (closed) return;
    const time = video.currentTime;
    if (time !== lastTime) {
      lastProgress = performance.now();
      lastTime = time;
    } else if (performance.now() - lastProgress > 8000) {
      fallback();
      return;
    }
    while (video.readyState >= 2) {
      const next = changes[0];
      if (!next || next.start > time) break;
      changes.shift();
      presented(next.pattern);
    }
    if (!buffer || buffer.updating) return;
    try {
      const ranges = buffer.buffered;
      const end = ranges.length ? ranges.end(ranges.length - 1) : 0;
      if (ranges.length && time - ranges.start(0) > 30) {
        buffer.remove(0, time - 10);
        return;
      }
      // Give the hardware enough data to start, then only queue the next color briefly ahead.
      if (end - time > (time === 0 ? 2 : 0.5)) return;
      const pattern = requested();
      const changed = pattern !== lastPattern;
      if (changed) fragmentIndex = 0;
      const cycle = cycles[pattern];
      const patchIndex = Math.floor(fragmentIndex / fragmentsPerPatch);
      const color = cycle ? cycle[patchIndex % cycle.length] : pattern;
      if (!color) throw Error(`Empty calibration cycle: ${pattern}`);
      buffer.appendBuffer(fragment(color));
      if (changed) changes.push({ start: end, pattern });
      lastPattern = pattern;
      fragmentIndex++;
    } catch {
      fallback();
    }
  }
  function open() {
    try {
      buffer = source.addSourceBuffer(mime);
      buffer.mode = 'sequence';
      buffer.addEventListener('error', fallback);
      buffer.addEventListener('updateend', pump);
      pump();
    } catch {
      fallback();
    }
  }
  source.addEventListener('sourceopen', open);
  video.src = url;
  const timer = window.setInterval(pump, 80);
  return () => {
    closed = true;
    clearInterval(timer);
    source.removeEventListener('sourceopen', open);
    buffer?.removeEventListener('error', fallback);
    buffer?.removeEventListener('updateend', pump);
    URL.revokeObjectURL(url);
  };
}

export function supportsPatternStream() {
  return (
    typeof MediaSource !== 'undefined' && MediaSource.isTypeSupported(mime)
  );
}
