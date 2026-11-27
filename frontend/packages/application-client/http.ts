import {
  colorsEventTypes,
  previewEventTypes,
  sessions,
} from '@prysm/contracts/constants';
import type {
  CaptureSettings,
  OutputSettings,
} from '@prysm/contracts/application';
import type { State as ApplicationStateReading } from '@prysm/contracts/application-reading';
import {
  isApplicationStateReading,
  isLightingFailureReading,
  isFailure,
  isFutureFailure,
  isSettingsReplaced,
  isCandidate,
  isModes,
  isFutureModes,
  isPreviewEvent,
  isColorsEvent,
} from '@prysm/contracts/validate/application';
import {
  ApplicationError,
  type ApplicationClient,
  type ApplicationSnapshot,
  type CaptureModesClient,
  type ColorsClient,
  type ColorsEvent,
  type DiscoveryClient,
  failureSections,
  isObject,
  known,
  type LightingFailure,
  lightingTargets,
  unreachable,
  type PreviewClient,
  type PreviewEvent,
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
const failure = (
  kind: 'unavailable' | 'forbidden' | 'internal',
  message: string,
) => new ApplicationError({ kind, message });

/**
 * A reply this page can't read: a defect, not a lost connection. A page
 * older than Prysm is the usual cause, and a reload brings the current one.
 */
const unreadable = (what: string) =>
  failure(
    'internal',
    `This page can’t read ${what} from Prysm. Reload the page.`,
  );

/** Decode the server's failure body; any other status is `unreadable`. */
async function responseError(response: Response): Promise<ApplicationError> {
  const body: unknown = await response.json().catch(() => null);
  if (isFailure(body)) {
    if (body.kind !== 'adapter') return new ApplicationError(body);
    const { message, section } = body;
    const side = failureSideOf(section, undefined);
    return new ApplicationError({
      kind: 'adapter',
      message,
      ...(side && { section: side }),
    });
  }
  if (isFutureFailure(body)) {
    const { message, section, field } = body;
    const side = failureSideOf(section, field);
    return side
      ? new ApplicationError({ kind: 'adapter', message, section: side })
      : new ApplicationError({
          kind: response.status < 500 ? 'bad_request' : 'internal',
          message,
        });
  }
  if (response.status === 403)
    return failure('forbidden', 'Prysm refuses requests from this page.');
  return unreadable(`a ${response.status} reply`);
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
export function httpApplicationClient({
  baseUrl = '',
  fetch: fetcher = fetch,
}: HttpClientOptions = {}): ApplicationClient<CaptureSettings, OutputSettings> {
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
        'Prysm didn’t answer. Check the current state before retrying.',
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
      if (!isSettingsReplaced(reply))
        throw failure('unavailable', 'Invalid settings reply.');
      return reply.revision;
    },
    observe: async (signal) => {
      const stream = await openEvents(fetcher, api, 'state', signal, readState);
      try {
        const first = await stream.events.next();
        if (first.done)
          throw failure('unavailable', 'State stream ended before any state.');
        return {
          initial: first.value,
          updates: stream.events,
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

/** The browser portal's live views and on-demand device lookups. */
export function httpPortalClient({
  baseUrl = '',
  fetch: fetcher = fetch,
}: HttpClientOptions = {}): PreviewClient &
  ColorsClient &
  DiscoveryClient &
  CaptureModesClient {
  const api = `${baseUrl.replace(/\/+$/, '')}/api/v1`;

  async function query(
    signal: AbortSignal | undefined,
    send: (options: RequestInit) => Promise<Response>,
  ): Promise<unknown> {
    let response: Response;
    try {
      response = await send({
        signal: signal ?? timeoutSignal(COMMAND_TIMEOUT_MS),
        cache: 'no-store',
        redirect: 'error',
      });
    } catch {
      throw failure('unavailable', 'Prysm didn’t answer.');
    }
    if (!response.ok) throw await responseError(response);
    return response.json().catch(() => null);
  }

  return {
    preview: (signal) =>
      openEvents(fetcher, api, 'preview', signal, decodePreview),
    colors: (signal) =>
      openEvents(fetcher, api, 'colors', signal, decodeColors),
    candidates: async (section, kind, signal) => {
      const found = await query(signal, (options) =>
        fetcher(
          `${api}/candidates/${section}/${encodeURIComponent(kind)}`,
          options,
        ),
      );
      if (!Array.isArray(found) || !found.every(isCandidate))
        throw failure('unavailable', 'Invalid candidates reply.');
      return found;
    },
    modes: async (capture, signal) => {
      const modes = await query(signal, (options) =>
        fetcher(`${api}/capture/modes`, {
          ...options,
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(capture),
        }),
      );
      if (isModes(modes)) return modes;
      if (isFutureModes(modes)) return { status: 'unknown' };
      throw failure('unavailable', 'Invalid capture modes reply.');
    },
  };
}

/** A stream's failure: one this page did not make is a lost connection. */
function streamError(error: unknown) {
  return error instanceof ApplicationError
    ? error
    : failure('unavailable', 'Can’t reach Prysm.');
}

/** Open an SSE stream; closing it or aborting `signal` cancels the request. */
async function openEvents<T>(
  fetcher: typeof fetch,
  api: string,
  name: 'state' | 'colors' | 'preview',
  signal: AbortSignal,
  decode: (value: unknown) => Decoded<T>,
) {
  const abort = new AbortController();
  const stop = () => abort.abort();
  signal.addEventListener('abort', stop, { once: true });
  if (signal.aborted) stop();
  let events: AsyncGenerator<T> | undefined;
  const close = () => {
    signal.removeEventListener('abort', stop);
    stop();
    void events?.return(undefined).catch(() => {});
  };
  try {
    const response = await fetcher(`${api}/${name}/stream`, {
      signal: abort.signal,
      cache: 'no-store',
      redirect: 'error',
    });
    if (!response.ok) throw await responseError(response);
    if (
      !response.headers.get('content-type')?.startsWith('text/event-stream') ||
      !response.body
    )
      throw unreadable(`the ${name} stream`);
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

function failureSideOf(section: string | undefined, field: string | undefined) {
  return (
    known(failureSections, section ?? '', undefined) ??
    known(failureSections, field?.split('.')[0] ?? '', undefined)
  );
}
/**
 * A session's failure as this page reads it, or `null` when malformed
 * (`field` and `section` are text when present). A `section` other than
 * `capture` or `output` reads as absent. A kind newer than this page is
 * said by its side, from its `section` or else its `field`'s section:
 * `adapter` with that section, else `internal`.
 */
function readFailure(value: unknown): LightingFailure | null {
  if (!isLightingFailureReading(value)) return null;
  const { kind, message, field, section } = value;
  const side = failureSideOf(section, undefined);
  switch (kind) {
    case 'invalid':
      return field !== undefined
        ? { kind: 'invalid', message, field }
        : { kind: 'invalid', message };
    case 'adapter':
      return side
        ? { kind: 'adapter', message, section: side }
        : { kind: 'adapter', message };
    case 'internal':
      return { kind: 'internal', message };
    default: {
      const said = failureSideOf(section, field);
      return said
        ? { kind: 'adapter', message, section: said }
        : { kind: 'internal', message };
    }
  }
}

/** Interpret newer session states and preserve any diagnostics we can read. */
function readSession(
  session: ApplicationStateReading['session'],
): SessionReading | null {
  const state = known(sessions, session.state, 'unknown');
  const carries =
    session.failure === undefined ? undefined : readFailure(session.failure);
  // A known state's failure is whole; a newer state's malformed one is
  // dropped, the state kept.
  if (carries === null && state !== 'unknown') return null;
  const message =
    typeof session.message === 'string' ? session.message : undefined;
  // Absent stays absent: a value is omitted, never `undefined`.
  const carried = carries ? { failure: carries } : {};
  switch (state) {
    case 'stopped':
    case 'effects':
    case 'unanswered':
      return { state, ...carried };
    case 'screen_sync':
      return { state };
    case 'faulted':
      return message === undefined ? null : { state, message };
    case 'unknown':
      return {
        state,
        ...carried,
        ...(message === undefined ? {} : { message }),
      };
    default:
      return unreachable(state);
  }
}

/** Validate the snapshot before interpreting names from newer portals. */
export function readState(
  value: unknown,
): Decoded<ApplicationSnapshot<CaptureSettings, OutputSettings>> {
  if (!isApplicationStateReading(value)) return 'invalid';
  const session = readSession(value.session);
  if (!session) return 'invalid';
  return {
    event: {
      ...value,
      target: known(lightingTargets, value.target, 'unknown'),
      session,
    },
  };
}

/**
 * The state the portal puts in its page (the `prysm-state` data block), read
 * as a streamed state is, so the first render shows it. `null` without a
 * valid one, such as from a dev server, which serves the page as it is.
 */
export function embeddedState(
  page: Document = document,
): ApplicationSnapshot<CaptureSettings, OutputSettings> | null {
  const text = page.getElementById('prysm-state')?.textContent;
  if (!text) return null;
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  const decoded = readState(value);
  return typeof decoded === 'object' ? decoded.event : null;
}

/** A future event type can be skipped. A malformed event of a known type is an error. */
function streamEvent<T extends { type: string }>(
  value: unknown,
  valid: (input: unknown) => input is T,
  types: readonly T['type'][],
): Decoded<T> {
  if (valid(value)) return { event: value };
  if (!isObject(value) || typeof value.type !== 'string') return 'invalid';
  return types.some((type) => type === value.type) ? 'invalid' : 'unknown';
}

export function decodePreview(value: unknown): Decoded<PreviewEvent> {
  return streamEvent(value, isPreviewEvent, previewEventTypes);
}

export function decodeColors(value: unknown): Decoded<ColorsEvent> {
  return streamEvent(value, isColorsEvent, colorsEventTypes);
}

/** Bounded SSE decoder; split UTF-8 chunks, CRLF and comment lines are handled. */
async function* decodeEvents<T>(
  body: ReadableStream<Uint8Array>,
  signal: AbortSignal,
  decode: (value: unknown) => Decoded<T>,
  name: string,
): AsyncGenerator<T> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const cancel = () => {
    void reader.cancel().catch(() => {});
  };
  signal.addEventListener('abort', cancel, { once: true });
  let text = '';
  try {
    while (!signal.aborted) {
      const chunk = await reader.read().catch((error: unknown) => {
        throw streamError(error);
      });
      if (chunk.done) return;
      text += decoder.decode(chunk.value, { stream: true });
      if (text.length > EVENT_LIMIT)
        throw unreadable(`a ${name} event this large`);
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
        if (decoded === 'invalid') throw unreadable(`a ${name} event`);
        if (decoded !== 'unknown') yield decoded.event;
      }
    }
  } finally {
    signal.removeEventListener('abort', cancel);
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
