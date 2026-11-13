import { ApplicationError, type Failure } from '@prysm/application-client';

/** An open fake stream, shaped like the client ports' streams. */
export interface FakeStream<E> {
  events: AsyncIterable<E>;
  close(): void;
}

/**
 * In-memory server-sent event streams: counts opens and closes, delivers
 * scripted events to every open stream, and can refuse the next open.
 */
export function fakeEventStreams<E>() {
  const streams = new Set<{
    push: (event: E | null) => void;
  }>();
  let opened = 0;
  let closed = 0;
  let refusal: Failure | null = null;
  const connect = (signal: AbortSignal): Promise<FakeStream<E>> => {
    if (refusal) {
      const failure = refusal;
      refusal = null;
      return Promise.reject(new ApplicationError(failure));
    }
    opened++;
    const queue: (E | null)[] = [];
    let wake: (() => void) | undefined;
    let done = false;
    const stream = {
      push: (event: E | null) => {
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
  };
  return {
    connect,
    controls: {
      /** Streams currently open. */
      open: () => streams.size,
      opened: () => opened,
      closed: () => closed,
      send: (event: E) => streams.forEach((stream) => stream.push(event)),
      /** End every open stream from the server side. */
      end: () => streams.forEach((stream) => stream.push(null)),
      refuseNext: (failure: Failure) => {
        refusal = failure;
      },
    },
  };
}
