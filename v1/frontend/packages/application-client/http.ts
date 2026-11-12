import {
  ApplicationError,
  type ApplicationClient,
  type ApplicationSnapshot,
  type Failure,
  type PreviewClient,
  type PreviewEvent,
} from './index';

const COMMAND_TIMEOUT_MS = 20_000;
const EVENT_LIMIT = 2 * 1024 * 1024;

/** `AbortSignal.timeout` exists from Chromium 103; older TV browsers lack it. */
function timeoutSignal(ms: number): AbortSignal {
  if (typeof AbortSignal.timeout === 'function') return AbortSignal.timeout(ms);
  const controller = new AbortController();
  setTimeout(() => controller.abort(), ms);
  return controller.signal;
}

const failure = (kind: Failure['kind'], message: string) =>
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
      await (calibration ? command('calibration', calibration) : command('calibration'));
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
        validState,
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
      const stream = await openEvents(fetcher, url, signal, validPreview, 'preview');
      return {
        events: stream.events as AsyncIterable<PreviewEvent>,
        close: stream.close,
      };
    },
  };
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
  valid: (value: unknown) => boolean,
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
    events = decodeEvents(response.body, abort.signal, valid, name);
    return { events, close };
  } catch (error) {
    close();
    throw streamError(error);
  }
}

const targets = ['off', 'processed', 'device_native'];
const sessions = ['stopped', 'running', 'native', 'faulted'];

function validState(value: unknown): boolean {
  const state = value as Partial<ApplicationSnapshot<unknown, unknown>> | null;
  return (
    !!state &&
    typeof state.revision === 'number' &&
    targets.includes(state.target as string) &&
    sessions.includes(state.session?.state as string) &&
    !!state.settings &&
    typeof state.settings.processing === 'object' &&
    Array.isArray(state.surface_failures)
  );
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

function validPreview(value: unknown): boolean {
  const event = value as Record<string, unknown> | null;
  if (!event || typeof event !== 'object') return false;
  if (event.type === 'idle') return true;
  if (event.type === 'unavailable') return typeof event.message === 'string';
  if (event.type !== 'frame') return false;
  const frame = value as Partial<Extract<PreviewEvent, { type: 'frame' }>>;
  const viewport = frame.viewport;
  const colors = frame.colors;
  return (
    [frame.sequence, frame.width, frame.height, frame.source_width, frame.source_height].every(
      (size) => isInt(size) && (size as number) >= 0,
    ) &&
    typeof frame.jpeg === 'string' &&
    !!viewport &&
    [viewport.x, viewport.y, viewport.width, viewport.height].every(isInt) &&
    !!colors &&
    [colors.top, colors.right, colors.bottom, colors.left].every(isColors)
  );
}

/** Bounded SSE decoder; split UTF-8 chunks, CRLF and comment lines are handled. */
async function* decodeEvents(
  body: ReadableStream<Uint8Array>,
  signal: AbortSignal,
  valid: (value: unknown) => boolean,
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
        if (!valid(value))
          throw failure('unavailable', `Invalid ${name} event.`);
        yield value;
      }
    }
  } finally {
    signal.removeEventListener('abort', cancel);
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
