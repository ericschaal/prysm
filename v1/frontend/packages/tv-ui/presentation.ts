import {
  newRequestId,
  type CalibrationClient,
  type OperationId,
} from '@prysm/application-client';
import type { PatternPresenter } from '@prysm/webos-platform';

/** Presentation lifetime only. Coordinator-owned expiry must cover UI crashes. */
export function createPresentationController(
  client: CalibrationClient,
  presenter: PatternPresenter,
) {
  let generation = 0;
  let starting = false;
  let session: {
    value: string;
    instance: string;
    closeId: OperationId;
  } | null = null;
  let stopping: Promise<void> | null = null;
  async function stop() {
    generation++;
    if (stopping) return stopping;
    if (!session) {
      presenter.release();
      return;
    }
    stopping = cleanup().finally(() => {
      stopping = null;
    });
    return stopping;
  }
  async function cleanup() {
    const current = session;
    try {
      presenter.release();
      if (current) {
        await client.feedback({
          id: newRequestId(current.instance),
          session: current.value,
          playback: { kind: 'released' },
        });
      }
    } finally {
      if (current) {
        await client.close({
          id: current.closeId,
          session: current.value,
        });
      }
    }
    // Keep unconfirmed cleanup owned so an explicit stop can retry it.
    session = null;
  }
  async function start(id: OperationId) {
    if (starting || session || stopping)
      throw new Error('Calibration presentation is already open or opening.');
    starting = true;
    try {
      return await open(id);
    } finally {
      starting = false;
    }
  }
  async function open(id: OperationId) {
    const current = ++generation;
    const presentation = await client.open(id);
    session = {
      value: presentation.session,
      instance: id.instance,
      closeId: newRequestId(id.instance),
    };
    if (current !== generation) {
      await stop();
      return null;
    }
    try {
      const playback = await presenter.present(presentation.pattern);
      if (current !== generation) return null;
      await client.feedback({
        id: newRequestId(id.instance),
        session: presentation.session,
        playback,
      });
      if (current !== generation) return null;
      if (playback.kind !== 'ready') {
        await stop();
        throw new Error(
          playback.kind === 'failed'
            ? playback.message
            : 'Pattern playback was released.',
        );
      }
      return presentation;
    } catch (error) {
      if (current === generation) {
        try {
          await client.feedback({
            id: newRequestId(id.instance),
            session: presentation.session,
            playback: { kind: 'failed', message: String(error) },
          });
        } finally {
          await stop();
        }
      }
      throw error;
    }
  }
  return { start, stop };
}
