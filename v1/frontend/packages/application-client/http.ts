import {
  ApplicationError,
  type ApplicationClient,
  type ApplicationSnapshot,
  type Failure,
} from './index';

const COMMAND_TIMEOUT_MS = 20_000;
const EVENT_LIMIT = 2 * 1024 * 1024;

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
  async function command(path: string, body: unknown): Promise<Response> {
    let response: Response;
    try {
      response = await fetcher(`${api}/${path}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(COMMAND_TIMEOUT_MS),
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
      const abort = new AbortController();
      const stop = () => abort.abort();
      signal.addEventListener('abort', stop, { once: true });
      if (signal.aborted) stop();
      let states: AsyncGenerator<ApplicationSnapshot<C, O>> | undefined;
      const close = () => {
        signal.removeEventListener('abort', stop);
        stop();
        void states?.return(undefined).catch(() => {});
      };
      try {
        const response = await fetcher(`${api}/state/stream`, {
          signal: abort.signal,
          cache: 'no-store',
          redirect: 'error',
        });
        if (!response.ok) throw await responseError(response);
        if (
          !response.headers
            .get('content-type')
            ?.startsWith('text/event-stream') ||
          !response.body
        )
          throw failure('unavailable', 'Invalid state stream response.');
        states = decodeStates<C, O>(response.body, abort.signal);
        const first = await states.next();
        if (first.done)
          throw failure('unavailable', 'State stream ended before any state.');
        return { initial: first.value, updates: states, close };
      } catch (error) {
        close();
        if (error instanceof ApplicationError) throw error;
        throw failure('unavailable', 'Cannot reach the portal.');
      }
    },
  };
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

/** Bounded SSE decoder; split UTF-8 chunks, CRLF and comment lines are handled. */
async function* decodeStates<C, O>(
  body: ReadableStream<Uint8Array>,
  signal: AbortSignal,
): AsyncGenerator<ApplicationSnapshot<C, O>> {
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
        throw failure('unavailable', 'State event is too large.');
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
        const state: unknown = JSON.parse(data);
        if (!validState(state))
          throw failure('unavailable', 'Invalid state event.');
        yield state as ApplicationSnapshot<C, O>;
      }
    }
  } finally {
    signal.removeEventListener('abort', cancel);
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
