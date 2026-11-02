import createClient from 'openapi-fetch';
import type { components, paths } from './api.generated';

export type StateResponse = components['schemas']['StateResponse'];
export type Config = components['schemas']['Config'];
export type Target = components['schemas']['Target'];

export const client = createClient<paths>({
  baseUrl: window.location.origin,
  fetch: request => fetch(request, { signal: AbortSignal.timeout(5000) }),
});

export function unwrap<T>(result: { data?: T; error?: unknown; response: Response }): T {
  if (result.data !== undefined) return result.data;
  const error = result.error;
  const message = error && typeof error === 'object' && 'message' in error ? String(error.message) : `Request failed (${result.response.status}).`;
  throw Error(message);
}
