export interface EventStream<T> {
  events: AsyncIterable<T>;
  close(): void;
}

export interface SnapshotStream<T> {
  initial: T;
  updates: AsyncIterable<T>;
  close(): void;
}

/** Deliver one open stream until EOF or cancellation. A late acquisition is still released. */
export async function consumeStream<T>(
  opening: Promise<EventStream<T> | SnapshotStream<T>>,
  signal: AbortSignal,
  receive: (value: T) => void,
) {
  const stream = await opening;
  if (signal.aborted) {
    stream.close();
    return;
  }
  const close = () => stream.close();
  signal.addEventListener('abort', close, { once: true });
  try {
    if ('initial' in stream) receive(stream.initial);
    if (signal.aborted) return;
    const events = 'updates' in stream ? stream.updates : stream.events;
    for await (const event of events) {
      if (signal.aborted) return;
      receive(event);
      if (signal.aborted) return;
    }
  } finally {
    signal.removeEventListener('abort', close);
    if (!signal.aborted) close();
  }
}
