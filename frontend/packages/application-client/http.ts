import {
  ApplicationError,
  type ApplicationClient,
  type ApplicationSnapshot,
  type Candidate,
  type CaptureModesClient,
  type ColorsClient,
  type ColorsEvent,
  type DiscoveryClient,
  type Failure,
  type LightingFailure,
  unreachable,
  type LightingTarget,
  type Mode,
  type Modes,
  type PreviewClient,
  type PreviewEvent,
  type Session,
  type SessionReading,
} from './index';

/**
 * Above the longest command the portal waits for, which sets no deadline of
 * its own: Rust's `prysm_runtime::COMMAND_LIMIT` (a desktop host's default
 * stop deadline plus `prysm_ports::SOURCE_OPEN_LIMIT`), with room for the
 * network. The TV's own portal stops sooner, so it stays below that too.
 */
const COMMAND_TIMEOUT_MS = 30_000;
const EVENT_LIMIT = 2 * 1024 * 1024;

/** `AbortSignal.timeout` exists from Chromium 103; older TV browsers lack it. */
function timeoutSignal(ms: number): AbortSignal {
  if (typeof AbortSignal.timeout === 'function') return AbortSignal.timeout(ms);
  const controller = new AbortController();
  setTimeout(() => controller.abort(), ms);
  return controller.signal;
}

/** A failure this page makes itself: one that carries only its message. */
const failure = (kind: 'unavailable' | 'forbidden', message: string) =>
  new ApplicationError({ kind, message });

/** Decode the server's failure body; anything else becomes `unavailable`. */
async function responseError(response: Response): Promise<ApplicationError> {
  const body: unknown = await response.json().catch(() => null);
  const detail = body as Partial<Failure> | null;
  if (
    detail &&
    typeof detail.kind === 'string' &&
    typeof detail.message === 'string'
  )
    return new ApplicationError(detail as Failure);
  if (response.status === 403)
    return failure('forbidden', 'The portal refuses requests from this page.');
  return failure('unavailable', `Unexpected response ${response.status}.`);
}

export interface HttpClientOptions {
  /**
   * The portal's origin, such as `http://192.168.1.20:8787`, for a page served
   * elsewhere; that page's origin must be allowed by the portal
   * (`--allow-origin`). Default: the page's own origin.
   */
  baseUrl?: string;
  fetch?: typeof fetch;
}

/** Fetch transport for the portal API, which needs no credentials. */
export function httpApplicationClient<C, O>({
  baseUrl = '',
  fetch: fetcher = fetch,
}: HttpClientOptions = {}): ApplicationClient<C, O> {
  const api = `${baseUrl.replace(/\/+$/, '')}/api/v1`;
  /** `PUT` the body, or `DELETE` when there is none. */
  async function command(path: string, body?: unknown): Promise<Response> {
    let response: Response;
    try {
      response = await fetcher(`${api}/${path}`, {
        ...(body === undefined
          ? { method: 'DELETE' }
          : {
              method: 'PUT',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(body),
            }),
        signal: timeoutSignal(COMMAND_TIMEOUT_MS),
        cache: 'no-store',
        redirect: 'error',
      });
    } catch {
      // The command may still have run; the state stream shows its outcome.
      throw failure(
        'unavailable',
        'The portal did not answer. Check the current state before retrying.',
      );
    }
    if (!response.ok) throw await responseError(response);
    return response;
  }
  return {
    setTarget: async (target) => {
      await command('target', { target });
    },
    setCalibration: async (calibration) => {
      await (calibration
        ? command('calibration', calibration)
        : command('calibration'));
    },
    tryProcessing: async (processing) => {
      await command('processing/trial', processing);
    },
    replaceSettings: async (expected, settings) => {
      const reply: unknown = await (
        await command('settings', { expected, settings })
      ).json();
      const revision = (reply as { revision?: unknown } | null)?.revision;
      if (typeof revision !== 'number')
        throw failure('unavailable', 'Invalid settings reply.');
      return revision;
    },
    observe: async (signal) => {
      const stream = await openEvents(
        fetcher,
        `${api}/state/stream`,
        signal,
        decodeState,
        'state',
      );
      try {
        const first = await stream.events.next();
        if (first.done)
          throw failure('unavailable', 'State stream ended before any state.');
        return {
          initial: first.value as ApplicationSnapshot<C, O>,
          updates: stream.events as AsyncIterable<ApplicationSnapshot<C, O>>,
          close: stream.close,
        };
      } catch (error) {
        stream.close();
        throw streamError(error);
      }
    },
  };
}

/**
 * Whether anything answers at `baseUrl`, ignoring CORS: an opaque `no-cors`
 * request for the portal page. A page that may not use the API (its origin is
 * not allowed) still gets an answer, which tells it from an unreachable portal.
 */
export async function probePortal(
  baseUrl: string,
  fetcher: typeof fetch = fetch,
): Promise<boolean> {
  try {
    await fetcher(`${baseUrl.replace(/\/+$/, '')}/`, {
      mode: 'no-cors',
      cache: 'no-store',
      signal: timeoutSignal(5_000),
    });
    return true;
  } catch {
    return false;
  }
}

/**
 * The portal's preview stream. Each open stream is a viewer the portal counts
 * (at most 4; more are refused as `busy`), so close it when it is not shown.
 */
export function httpPreviewClient({
  baseUrl = '',
  fetch: fetcher = fetch,
}: HttpClientOptions = {}): PreviewClient {
  const url = `${baseUrl.replace(/\/+$/, '')}/api/v1/preview/stream`;
  return {
    preview: async (signal) => {
      const stream = await openEvents(
        fetcher,
        url,
        signal,
        decodePreview,
        'preview',
      );
      return {
        events: stream.events as AsyncIterable<PreviewEvent>,
        close: stream.close,
      };
    },
  };
}

/**
 * The portal's colors stream. Each open stream is a listener the portal counts
 * (at most 8; more are refused as `busy`), so close it when it is not shown.
 */
export function httpColorsClient({
  baseUrl = '',
  fetch: fetcher = fetch,
}: HttpClientOptions = {}): ColorsClient {
  const url = `${baseUrl.replace(/\/+$/, '')}/api/v1/colors/stream`;
  return {
    colors: async (signal) => {
      const stream = await openEvents(
        fetcher,
        url,
        signal,
        decodeColors,
        'colors',
      );
      return {
        events: stream.events as AsyncIterable<ColorsEvent>,
        close: stream.close,
      };
    },
  };
}

/** The portal's `GET /api/v1/candidates/{section}/{kind}`. */
export function httpDiscoveryClient({
  baseUrl = '',
  fetch: fetcher = fetch,
}: HttpClientOptions = {}): DiscoveryClient {
  const api = `${baseUrl.replace(/\/+$/, '')}/api/v1/candidates`;
  return {
    candidates: async (section, kind, signal) => {
      let response: Response;
      try {
        response = await fetcher(
          `${api}/${section}/${encodeURIComponent(kind)}`,
          {
            signal: signal ?? timeoutSignal(COMMAND_TIMEOUT_MS),
            cache: 'no-store',
            redirect: 'error',
          },
        );
      } catch {
        throw failure('unavailable', 'The portal did not answer.');
      }
      if (!response.ok) throw await responseError(response);
      const found: unknown = await response.json().catch(() => null);
      if (!Array.isArray(found) || !found.every(validCandidate))
        throw failure('unavailable', 'Invalid candidates reply.');
      return found;
    },
  };
}

/** The portal's `POST /api/v1/capture/modes`. */
export function httpCaptureModesClient({
  baseUrl = '',
  fetch: fetcher = fetch,
}: HttpClientOptions = {}): CaptureModesClient {
  const api = `${baseUrl.replace(/\/+$/, '')}/api/v1/capture/modes`;
  return {
    modes: async (capture, signal) => {
      let response: Response;
      try {
        response = await fetcher(api, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(capture),
          signal: signal ?? timeoutSignal(COMMAND_TIMEOUT_MS),
          cache: 'no-store',
          redirect: 'error',
        });
      } catch {
        throw failure('unavailable', 'The portal did not answer.');
      }
      if (!response.ok) throw await responseError(response);
      const modes: unknown = await response.json().catch(() => null);
      if (!validModes(modes))
        throw failure('unavailable', 'Invalid capture modes reply.');
      return modes;
    },
  };
}

function validModes(value: unknown): value is Modes {
  const modes = value as { status?: unknown; modes?: unknown } | null;
  if (modes?.status === 'unknown') return true;
  return (
    modes?.status === 'known' &&
    Array.isArray(modes.modes) &&
    modes.modes.every(validMode)
  );
}

function validMode(value: unknown): value is Mode {
  const mode = value as Partial<Record<keyof Mode, unknown>> | null;
  const span = (value: unknown) => {
    const span = value as {
      min?: unknown;
      max?: unknown;
      step?: unknown;
    } | null;
    return (
      Number.isSafeInteger(span?.min) &&
      Number.isSafeInteger(span?.max) &&
      Number.isSafeInteger(span?.step)
    );
  };
  return !!mode && span(mode.width) && span(mode.height) && span(mode.fps);
}

function validCandidate(value: unknown): value is Candidate {
  const candidate = value as Partial<Candidate> | null;
  return (
    !!candidate &&
    typeof candidate.label === 'string' &&
    (candidate.detail === undefined || typeof candidate.detail === 'string') &&
    (candidate.led_count === undefined ||
      Number.isSafeInteger(candidate.led_count)) &&
    !!candidate.fields &&
    typeof candidate.fields === 'object' &&
    !Array.isArray(candidate.fields)
  );
}

function streamError(error: unknown) {
  return error instanceof ApplicationError
    ? error
    : failure('unavailable', 'Cannot reach the portal.');
}

/** Open an SSE stream; closing it or aborting `signal` cancels the request. */
async function openEvents(
  fetcher: typeof fetch,
  url: string,
  signal: AbortSignal,
  decode: (value: unknown) => Decoded<unknown>,
  name: string,
) {
  const abort = new AbortController();
  const stop = () => abort.abort();
  signal.addEventListener('abort', stop, { once: true });
  if (signal.aborted) stop();
  let events: AsyncGenerator<unknown> | undefined;
  const close = () => {
    signal.removeEventListener('abort', stop);
    stop();
    void events?.return(undefined).catch(() => {});
  };
  try {
    const response = await fetcher(url, {
      signal: abort.signal,
      cache: 'no-store',
      redirect: 'error',
    });
    if (!response.ok) throw await responseError(response);
    if (
      !response.headers.get('content-type')?.startsWith('text/event-stream') ||
      !response.body
    )
      throw failure('unavailable', `Invalid ${name} stream response.`);
    events = decodeEvents(response.body, abort.signal, decode, name);
    return { events, close };
  } catch (error) {
    close();
    throw streamError(error);
  }
}

/**
 * An event as decoded: its value, `unknown` (a type newer than this page,
 * skipped) or `invalid` (malformed, which ends the stream).
 */
export type Decoded<T> = { event: T } | 'unknown' | 'invalid';

type Fields = Partial<Record<string, unknown>>;
const isObject = (value: unknown): value is Fields =>
  !!value && typeof value === 'object' && !Array.isArray(value);

/** Keyed by every value of the generated union, so the compiler keeps them in step. */
const targets: Record<LightingTarget, true> = {
  off: true,
  screen_sync: true,
  effects: true,
};
const sessions: Record<Session['state'], true> = {
  stopped: true,
  screen_sync: true,
  effects: true,
  faulted: true,
};
/** `raw` if it is one of `values`, else `fallback`: a value newer than this page. */
function known<T extends string, F extends string>(
  values: Record<T, true>,
  raw: string,
  fallback: F,
): T | F {
  return Object.keys(values).includes(raw) ? (raw as T) : fallback;
}

/**
 * A session's failure as this page reads it, or `null` when malformed
 * (`field` and `section` are text when present). A `section` other than
 * `capture` or `output` reads as absent; a kind newer than this page is
 * said by its side: `adapter` with a known section, else `internal`.
 */
function decodeFailure(value: unknown): LightingFailure | null {
  if (!isObject(value)) return null;
  const { kind, message, field, section } = value;
  if (
    typeof kind !== 'string' ||
    typeof message !== 'string' ||
    (field !== undefined && typeof field !== 'string') ||
    (section !== undefined && typeof section !== 'string')
  )
    return null;
  const side =
    section === 'capture' || section === 'output' ? section : undefined;
  switch (kind) {
    case 'invalid':
      return typeof field === 'string'
        ? { kind: 'invalid', message, field }
        : { kind: 'invalid', message };
    case 'adapter':
      return side
        ? { kind: 'adapter', message, section: side }
        : { kind: 'adapter', message };
    case 'internal':
      return { kind: 'internal', message };
    default:
      return side
        ? { kind: 'adapter', message, section: side }
        : { kind: 'internal', message };
  }
}

/**
 * A state as this page reads it. Its structure is checked; a value newer
 * than this page in a known field has one reading: an unknown target, or
 * an unknown session state, is `unknown` (never off or stopped), and the
 * failure or message such a session carries is kept. Fields not checked
 * here reach the views as sent, and each has its own reading there.
 */
export function decodeState(
  value: unknown,
): Decoded<ApplicationSnapshot<unknown, unknown>> {
  if (!isObject(value)) return 'invalid';
  const { session, settings } = value;
  if (
    typeof value.version !== 'string' ||
    typeof value.revision !== 'number' ||
    typeof value.target !== 'string' ||
    !isObject(session) ||
    typeof session.state !== 'string' ||
    !isObject(value.device) ||
    !isObject(settings) ||
    !isObject(settings.processing) ||
    !Array.isArray(value.surface_failures)
  )
    return 'invalid';
  const state = known(sessions, session.state, 'unknown');
  const carries =
    session.failure === undefined ? undefined : decodeFailure(session.failure);
  // A known state's failure is whole; a newer state's malformed one is
  // dropped, the state kept.
  if (carries === null && state !== 'unknown') return 'invalid';
  const message =
    typeof session.message === 'string' ? session.message : undefined;
  // Absent stays absent: a value is omitted, never `undefined`.
  const carried = carries ? { failure: carries } : {};
  let decoded: SessionReading;
  switch (state) {
    case 'stopped':
      decoded = { state: 'stopped', ...carried };
      break;
    case 'effects':
      decoded = { state: 'effects', ...carried };
      break;
    case 'screen_sync':
      decoded = { state: 'screen_sync' };
      break;
    case 'faulted':
      if (message === undefined) return 'invalid';
      decoded = { state: 'faulted', message };
      break;
    case 'unknown':
      decoded = {
        state: 'unknown',
        ...carried,
        ...(message === undefined ? {} : { message }),
      };
      break;
    default:
      return unreachable(state);
  }
  return {
    event: {
      ...(value as unknown as ApplicationSnapshot<unknown, unknown>),
      target: known(targets, value.target, 'unknown'),
      session: decoded,
    },
  };
}

const isInt = (value: unknown) => Number.isInteger(value);
const isColors = (value: unknown) =>
  Array.isArray(value) &&
  value.every(
    (color) =>
      Array.isArray(color) &&
      color.length === 3 &&
      color.every((byte) => isInt(byte) && byte >= 0 && byte <= 255),
  );

/** A preview event; one of a type newer than this page is skipped. */
export function decodePreview(value: unknown): Decoded<PreviewEvent> {
  if (!isObject(value) || typeof value.type !== 'string') return 'invalid';
  const event = value as unknown as PreviewEvent;
  switch (value.type) {
    case 'idle':
      return { event };
    case 'unavailable':
      return typeof value.message === 'string' ? { event } : 'invalid';
    case 'frame': {
      const frame = value as unknown as Partial<
        Extract<PreviewEvent, { type: 'frame' }>
      >;
      const viewport = frame.viewport;
      const colors = frame.colors;
      const valid =
        [
          frame.sequence,
          frame.width,
          frame.height,
          frame.source_width,
          frame.source_height,
        ].every((size) => isInt(size) && (size as number) >= 0) &&
        typeof frame.jpeg === 'string' &&
        !!viewport &&
        [viewport.x, viewport.y, viewport.width, viewport.height].every(
          isInt,
        ) &&
        !!colors &&
        [colors.top, colors.right, colors.bottom, colors.left].every(isColors);
      return valid ? { event } : 'invalid';
    }
    default:
      return 'unknown';
  }
}

/** A colors event; one of a type newer than this page is skipped. */
export function decodeColors(value: unknown): Decoded<ColorsEvent> {
  if (!isObject(value) || typeof value.type !== 'string') return 'invalid';
  const event = value as unknown as ColorsEvent;
  switch (value.type) {
    case 'idle':
      return { event };
    case 'colors':
      return [value.top, value.right, value.bottom, value.left].every(isColors)
        ? { event }
        : 'invalid';
    default:
      return 'unknown';
  }
}

/** Bounded SSE decoder; split UTF-8 chunks, CRLF and comment lines are handled. */
async function* decodeEvents(
  body: ReadableStream<Uint8Array>,
  signal: AbortSignal,
  decode: (value: unknown) => Decoded<unknown>,
  name: string,
): AsyncGenerator<unknown> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const cancel = () => {
    void reader.cancel().catch(() => {});
  };
  signal.addEventListener('abort', cancel, { once: true });
  let text = '';
  try {
    while (!signal.aborted) {
      const chunk = await reader.read();
      if (chunk.done) return;
      text += decoder.decode(chunk.value, { stream: true });
      if (text.length > EVENT_LIMIT)
        throw failure('unavailable', `The ${name} event is too large.`);
      let boundary: RegExpExecArray | null;
      while ((boundary = /\r?\n\r?\n/.exec(text))) {
        const event = text.slice(0, boundary.index);
        text = text.slice(boundary.index + boundary[0].length);
        const data = event
          .split(/\r?\n/)
          .filter((line) => line.startsWith('data:'))
          .map((line) => line.slice(5).trimStart())
          .join('\n');
        if (!data) continue;
        let value: unknown;
        try {
          value = JSON.parse(data);
        } catch {
          value = null;
        }
        const decoded = decode(value);
        if (decoded === 'invalid')
          throw failure('unavailable', `Invalid ${name} event.`);
        if (decoded !== 'unknown') yield decoded.event;
      }
    }
  } finally {
    signal.removeEventListener('abort', cancel);
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
