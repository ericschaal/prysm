import { useRef, type RefObject } from 'react';
import type { PreviewClient, PreviewFrame } from '@prysm/application-client';
import { usePageVisible } from './live-colors';
import { LightsIcon, VideoIcon } from './path-icons';
import { usePreviewStream } from './preview-stream';
import { ChevronIcon } from './rows';
import type { Status } from './section-state';

/** The picture alone, filling the thumbnail: no colours around it. */
function drawPicture(
  canvas: HTMLCanvasElement,
  image: CanvasImageSource | null,
  frame: PreviewFrame,
) {
  if (canvas.width !== frame.width) canvas.width = frame.width;
  if (canvas.height !== frame.height) canvas.height = frame.height;
  const context = canvas.getContext('2d');
  if (context && image)
    context.drawImage(image, 0, 0, frame.width, frame.height);
}

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
 * of words; below it, while it fails, Prysm's own words and `hint`. With
 * `open`, the card's line is the way to the large preview. `brief`: the
 * failure is told elsewhere (the save bar), so not here.
 */
export function SectionStatus({
  section,
  status,
  preview,
  open,
  hint,
  brief = false,
}: {
  section: 'capture' | 'output';
  status: Status;
  preview?: PreviewClient;
  /** What opening the card shows, such as `See the picture`, and how. */
  open?: { label: string; onOpen: () => void };
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
  const content = (
    <>
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
        {open ? <span className="visually-hidden">{open.label}</span> : null}
      </span>
    </>
  );
  return (
    <div
      className="section-status"
      data-section={section}
      data-tone={status.tone}
      role="status"
      aria-label={section === 'capture' ? 'Video source now' : 'Lights now'}
    >
      {open ? (
        <button
          type="button"
          className="section-status-main"
          onClick={open.onOpen}
        >
          {content}
          <ChevronIcon />
        </button>
      ) : (
        <div className="section-status-main">{content}</div>
      )}
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
