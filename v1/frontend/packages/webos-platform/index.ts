import type { TvHostCommand, TvHostStatus } from '@prysm/contracts/tv-host';
import type {
  PatternRequest,
  PlaybackFeedback,
} from '@prysm/application-client';

export interface TvHostClient {
  status(): Promise<TvHostStatus>;
  command(command: TvHostCommand): Promise<void>;
}

/** Readiness means actual video playback, not that a React element was mounted. */
export interface PatternPresenter {
  present(pattern: PatternRequest): Promise<PlaybackFeedback>;
  /** Invalidates pending playback as well as visible playback; late readiness must not restart it. */
  release(): void;
}

export function unavailableTvHost(): TvHostClient {
  const unavailable = (): Promise<never> =>
    Promise.reject(new Error('webOS platform bridge is not implemented.'));
  return { status: unavailable, command: unavailable };
}

export function unavailablePatternPresenter(): PatternPresenter {
  return {
    present: () =>
      Promise.reject(new Error('Native pattern playback is not implemented.')),
    release: () => {},
  };
}
