import { useRef, type RefObject } from 'react';
import type { AdapterSide, PreviewClient } from '@prysm/application-client';
import { usePageVisible } from './live-colors';
import { LightsIcon, VideoIcon } from './path-icons';
import { drawPicture } from './preview-canvas';
import { usePreviewStream } from './preview-stream';
import type { Status } from './section-state';

/** The live picture's size, while it streams into `canvasRef`. */
function usePictureSize(
  client: PreviewClient | undefined,
  active: boolean,
  canvasRef: RefObject<HTMLCanvasElement | null>,
) {
  const visible = usePageVisible();
  const view = usePreviewStream(
    client ?? idlePreview,
    !!client && active && visible,
    0,
    canvasRef,
    drawPicture,
  );
  return view.kind === 'frame' ? view.source : null;
}

/** Stands in while there is no preview client; never opened. */
const idlePreview: PreviewClient = {
  preview: () => Promise.reject(new Error('No preview.')),
};

/**
 * Whether the section works, at the top of its editor: a small live picture
 * (the video source, only while frames flow) or the lights' mark, and a line
 * of words; below it, while it fails, Prysm's own words and `hint`. `brief`:
 * the failure is told elsewhere (the save bar), so not here.
 */
export function SectionStatus({
  section,
  status,
  preview,
  hint,
  brief = false,
}: {
  section: AdapterSide;
  status: Status;
  preview?: PreviewClient;
  /** What to check while it fails, from the saved kind. */
  hint?: string;
  brief?: boolean;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const picture = !!status.picture;
  const size = usePictureSize(preview, picture, canvasRef);
  const showing = picture && size !== null;
  const mark = section === 'capture' ? <VideoIcon /> : <LightsIcon />;
  const detail = picture && size ? `${size} · ${status.detail}` : status.detail;
  // Why it fails and what to check, under the card's line, full width.
  const more = brief
    ? []
    : [status.details, hint].filter((line) => line !== undefined);
  return (
    <div
      className="section-status"
      data-section={section}
      data-tone={status.tone}
      role="status"
      aria-label={section === 'capture' ? 'Video source now' : 'Lights now'}
    >
      <div className="section-status-main">
        <span className="section-status-tile" data-picture={showing}>
          {section === 'capture' ? (
            <canvas ref={canvasRef} aria-hidden="true" hidden={!showing} />
          ) : null}
          {showing ? null : mark}
        </span>
        <span className="section-status-text">
          <span className="section-status-title">{status.title}</span>
          {detail ? (
            <span className="section-status-detail">{detail}</span>
          ) : null}
        </span>
      </div>
      {more.length ? (
        <div className="section-status-more">
          {more.map((line) => (
            <p key={line}>{line}</p>
          ))}
        </div>
      ) : null}
    </div>
  );
}
