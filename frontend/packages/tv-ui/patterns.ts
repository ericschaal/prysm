/**
 * Calibration patterns as real video on the TV's video plane, never drawn by
 * the app: one Media Source Extensions stream of half-second H.264 fragments
 * (each a complete MP4 starting on a keyframe), appended in sequence mode, so
 * switching patterns never restarts the TV's video pipeline.
 *
 * `pattern-fragments.json` holds the clips the earlier TV app shipped (its
 * generator, scripts/calibration-patterns.py, is in the tag
 * `pre-1.0-consolidation`), without `gray-64`.
 */
import type { Pattern } from '@prysm/application-client';
import bundled from './pattern-fragments.json';

/** The fragments' codec: constrained baseline H.264, level 3.0. */
export const PATTERN_TYPE = 'video/mp4; codecs="avc1.42c01e"';
/** Plays while no pattern is chosen, so the video pipeline is already running. */
export const WAITING_PATTERN: Pattern = 'gray_192';
/**
 * No playback progress for this long ends the stream as failed. A guess: no
 * stall of the TV's media pipeline was measured.
 */
export const STALL_MS = 8000;

const fragments: Record<Pattern, string> = bundled;
export const patternNames = Object.keys(fragments) as Pattern[];

/** The seam between the TV UI and the video: tests pass a fake. */
export interface PatternPlayer {
  /** The TV can play the pattern stream. */
  readonly supported: boolean;
  /**
   * Stream `pattern()` (read again for every fragment) into `video` until the
   * returned stop is called; `failed` is called at most once, after which the
   * stream has stopped.
   */
  play(
    video: HTMLVideoElement,
    pattern: () => Pattern,
    failed: () => void,
  ): () => void;
}

interface MediaWindow {
  MediaSource?: typeof MediaSource;
  URL: typeof URL;
}
const media = () => window as unknown as MediaWindow;

const decoded = new Map<Pattern, Uint8Array<ArrayBuffer>>();
/** One fragment of `pattern`. */
function fragment(pattern: Pattern): Uint8Array<ArrayBuffer> {
  let bytes = decoded.get(pattern);
  if (!bytes) {
    bytes = Uint8Array.from(atob(fragments[pattern]), (c) => c.charCodeAt(0));
    decoded.set(pattern, bytes);
  }
  return bytes;
}

export function streamPatterns(
  video: HTMLVideoElement,
  pattern: () => Pattern,
  failed: () => void,
): () => void {
  const { MediaSource: Source, URL: Url } = media();
  const source = new Source!();
  const url = Url.createObjectURL(source);
  let buffer: SourceBuffer | undefined;
  let closed = false;
  let lastTime = -1;
  let stall: ReturnType<typeof setTimeout> | undefined;

  /**
   * Append when the buffer has room: at each finished append (`updateend`),
   * as playback moves (`timeupdate`) and when it runs out (`waiting`).
   * Progress restarts the stall wait.
   */
  function pump() {
    if (closed) return;
    const time = video.currentTime;
    if (time !== lastTime) {
      lastTime = time;
      clearTimeout(stall);
      stall = setTimeout(fail, STALL_MS);
    }
    if (!buffer || buffer.updating) return;
    try {
      const ranges = buffer.buffered;
      const end = ranges.length ? ranges.end(ranges.length - 1) : 0;
      // Drop what has played, keeping the last ten seconds.
      if (ranges.length && time - ranges.start(0) > 30) {
        buffer.remove(0, time - 10);
        return;
      }
      if (end - time > (time === 0 ? 2 : 0.5)) return;
      buffer.appendBuffer(fragment(pattern()));
    } catch {
      fail();
    }
  }
  function open() {
    try {
      buffer = source.addSourceBuffer(PATTERN_TYPE);
      buffer.mode = 'sequence';
      buffer.addEventListener('error', fail);
      buffer.addEventListener('updateend', pump);
      pump();
    } catch {
      fail();
    }
  }
  function stop() {
    if (closed) return;
    closed = true;
    clearTimeout(stall);
    source.removeEventListener('sourceopen', open);
    buffer?.removeEventListener('error', fail);
    buffer?.removeEventListener('updateend', pump);
    video.removeEventListener('error', fail);
    video.removeEventListener('timeupdate', pump);
    video.removeEventListener('waiting', pump);
    video.pause();
    video.removeAttribute('src');
    video.load();
    Url.revokeObjectURL(url);
  }
  function fail() {
    if (closed) return;
    stop();
    failed();
  }
  source.addEventListener('sourceopen', open);
  video.addEventListener('error', fail);
  video.addEventListener('timeupdate', pump);
  // Out of data before the next `timeupdate`: append at once.
  video.addEventListener('waiting', pump);
  video.src = url;
  stall = setTimeout(fail, STALL_MS);
  const playing = video.play() as Promise<void> | undefined;
  // Stopping while play() is pending rejects it with AbortError: not a failure.
  playing?.catch((reason: unknown) => {
    if ((reason as { name?: string } | undefined)?.name !== 'AbortError')
      fail();
  });
  return stop;
}

/** The TV's own MSE player. */
export const msePatternPlayer: PatternPlayer = {
  get supported() {
    const { MediaSource: Source } = media();
    return typeof Source === 'function' && Source.isTypeSupported(PATTERN_TYPE);
  },
  play: streamPatterns,
};
