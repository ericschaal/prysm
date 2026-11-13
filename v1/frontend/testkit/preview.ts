import {
  ApplicationError,
  type Failure,
  type PreviewClient,
  type PreviewEvent,
} from '@prysm/application-client';

/**
 * In-memory preview streams: counts opens and closes, delivers scripted
 * events to every open stream, and can refuse the next open.
 */
export function fakePreviewClient() {
  const streams = new Set<{
    push: (event: PreviewEvent | null) => void;
  }>();
  let opened = 0;
  let closed = 0;
  let refusal: Failure | null = null;
  const client: PreviewClient = {
    preview: (signal) => {
      if (refusal) {
        const failure = refusal;
        refusal = null;
        return Promise.reject(new ApplicationError(failure));
      }
      opened++;
      const queue: (PreviewEvent | null)[] = [];
      let wake: (() => void) | undefined;
      let done = false;
      const stream = {
        push: (event: PreviewEvent | null) => {
          queue.push(event);
          wake?.();
        },
      };
      function close() {
        if (done) return;
        done = true;
        closed++;
        streams.delete(stream);
        wake?.();
      }
      signal.addEventListener('abort', close, { once: true });
      streams.add(stream);
      return Promise.resolve({
        close,
        events: (async function* () {
          while (!done) {
            if (!queue.length)
              await new Promise<void>((resolve) => {
                wake = resolve;
              });
            while (queue.length && !done) {
              const event = queue.shift()!;
              // `null` ends the stream, as a portal shutting down does.
              if (event === null) return close();
              yield event;
            }
          }
        })(),
      });
    },
  };
  return {
    client,
    /** Streams currently open. */
    open: () => streams.size,
    opened: () => opened,
    closed: () => closed,
    send: (event: PreviewEvent) =>
      streams.forEach((stream) => stream.push(event)),
    /** End every open stream from the server side. */
    end: () => streams.forEach((stream) => stream.push(null)),
    refuseNext: (failure: Failure) => {
      refusal = failure;
    },
  };
}
