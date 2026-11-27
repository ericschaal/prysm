import { useId, type ReactNode } from 'react';
import {
  REFUSED_VIDEO,
  type FailureSection,
  type Input,
  type Picture,
} from '@prysm/application-client';
import {
  ledCount,
  unbroken,
  type AdapterFact,
  type LedEdges,
  type Transport,
  type SetupPart,
} from './adapters';
import { useDeviceName } from './device-name';
import { LedDrawing } from './led-drawing';
import { LightsIcon, PrysmIcon, VideoIcon } from './path-icons';
import { PreviewDrawing, type PreviewSource } from './preview';
import { RowLink } from './rows';

/**
 * What the path is doing, from the state: frames flowing through Prysm,
 * screen sync on without a picture, the lights' own effects (Prysm not in
 * the path), a failure (at the source, at the lights, or somewhere on the
 * way; also with the lights off), or off.
 */
export type PathState =
  | { kind: 'off' }
  | { kind: 'flowing'; input: Input }
  | {
      kind: 'waiting';
      /** Why no picture: none given, or the TV refuses to share it. */
      picture: Exclude<Picture, 'arriving'>;
    }
  | { kind: 'effects' }
  | { kind: 'failed'; at: FailureSection | null };

/** The Video source fact while no picture arrives, by why. */
const waitingFact: Record<Exclude<Picture, 'arriving'>, string> = {
  quiet: 'Waiting for picture',
  refused: REFUSED_VIDEO,
};

/** One stop on the path: its role (for screen readers), name and one fact. */
function Stop({
  title,
  icon,
  name,
  fact,
  figure,
  failed,
  behind = false,
  instead = false,
}: {
  title: string;
  icon: ReactNode;
  name: string;
  fact?: string;
  figure?: { text: string; about: string };
  instead?: boolean;
  failed: boolean;
  behind?: boolean;
}) {
  const tooltip = instead ? fact : undefined;
  // A kind's qualifier ("(development only)") stays in the tooltip.
  const short = name.replace(/\s*\([^)]*\)$/, '');
  // The fact moved off the line, said once for screen readers.
  const hidden = figure !== undefined ? tooltip : undefined;
  const line = hidden ? undefined : fact;
  return (
    <li className="path-stop" data-failed={failed} data-behind={behind}>
      <span className="path-icon">{icon}</span>
      <span className="path-text" title={tooltip}>
        <span className="visually-hidden">{title}: </span>
        {/* Never broken at a hyphen ("WLED-Gledopto"). */}
        <strong className="path-name" title={short === name ? undefined : name}>
          {unbroken(short)}
        </strong>
        {hidden ? <span className="visually-hidden"> {hidden},</span> : null}
        {/* Always a line, so the stops stay aligned when one has no fact. */}
        <span className="path-fact">
          {line || figure ? (
            <>
              {line ? <span className="path-fact-text">{line}</span> : null}
              {figure ? (
                <span className="path-figure" title={figure.about}>
                  {/* The separator rides with the figure (flex drops loose spaces). */}
                  {line ? '\u00a0·\u00a0' : null}
                  {figure.text}
                  <span className="visually-hidden">: {figure.about}</span>
                </span>
              ) : null}
            </>
          ) : (
            '\u00a0'
          )}
        </span>
      </span>
    </li>
  );
}

/** Whole milliseconds, "<1 ms" below one. */
const millis = (ms: number) => `${ms < 1 ? '<1' : Math.round(ms)} ms`;

/**
 * What a link adds to the lag: "+88 ms", and "<1 ms" below one millisecond
 * (no "+": "+<1" reads as two operators).
 */
function delay(ms: number | undefined) {
  if (ms === undefined) return undefined;
  return ms < 1 ? millis(ms) : `+${millis(ms)}`;
}

/** A rate to whole frames, with the tenth only when it matters: `29.9 fps`. */
function frameRate(fps: number) {
  const whole = Math.round(fps);
  return `${Math.abs(fps - whole) < 0.05 ? whole : fps.toFixed(1)} fps`;
}

type LinkState = 'still' | 'flowing' | 'failed';

function Link({
  state,
  transport,
  numbers,
  about,
  behind = false,
}: {
  state: LinkState;
  behind?: boolean;
  transport?: Transport;
  numbers?: string;
  /** What the numbers measure, for a tooltip and screen readers. */
  about?: string;
}) {
  const [name, said] = transport ? transportNames[transport] : [];
  return (
    <li
      className="path-link"
      data-state={state}
      data-behind={behind}
      aria-hidden={!name && !numbers}
    >
      <span className="path-line">
        <span className="path-pulse" />
      </span>
      {/* Both always there, so nothing moves when they fill. */}
      <span className="path-label path-protocol" title={said}>
        {name}
        {said ? <span className="visually-hidden">: {said}</span> : null}
      </span>
      <span
        className="path-label path-numbers"
        title={numbers ? about : undefined}
      >
        {numbers}
        {numbers && about ? (
          <span className="visually-hidden">: {about}</span>
        ) : null}
      </span>
    </li>
  );
}

/**
 * How each transport is named on its link, and said in full (a tooltip and
 * for screen readers). Checked against the adapters: the remote capture's
 * stream is TCP, WLED's colours are DDP over UDP (port 4048).
 */
const transportNames: Record<Transport, [name: string, about: string]> = {
  local: ['Local', 'Video from a device on this computer'],
  file: ['File', 'Video from a file'],
  built_in: ['Built in', 'The TV’s own picture'],
  h264: ['H.264', 'H.264 over TCP'],
  raw: ['Raw', 'Raw video over TCP'],
  ddp: ['DDP', 'DDP over UDP'],
};

/** The facts as one quiet line: `640 × 480 · 30 fps`. */
const factLine = (facts: AdapterFact[], skip: string[] = []) =>
  facts
    .filter((fact) => !skip.includes(fact.label))
    .map((fact) => fact.value)
    .join(' · ') || undefined;

const ledCaption = (edges: LedEdges) =>
  `Seen from the front: ${edges.top} LEDs along the top, ${edges.right} on the right, ${edges.bottom} along the bottom and ${edges.left} on the left, ${ledCount(edges)} in all.`;

/**
 * Your setup, as the path the light takes: the video source, Prysm and the
 * lights, joined by two hairlines that carry what is measured (the picture
 * rate arriving) and light up while frames flow. Only what Prysm's state and
 * settings say (and discovery, for names); the forms are in Settings. Below
 * the path, the TV with its LEDs, and the live preview in its screen when
 * the host offers one.
 */
export function SetupCard({
  capture,
  output,
  path,
  calibration,
  preview,
  openSettings,
}: {
  capture: SetupPart;
  output: SetupPart;
  path: PathState;
  /** The active calibration's name, or `null` for no correction. */
  calibration: string | null;
  /** The live preview, when this host offers one. */
  preview?: PreviewSource;
  openSettings: () => void;
}) {
  const headingId = useId();
  const { edges } = output;
  const captureName = useDeviceName(capture);
  const outputName = useDeviceName(output);
  const failedAt = path.kind === 'failed' ? path.at : undefined;
  let into: LinkState = 'still';
  let out: LinkState = 'still';
  if (path.kind === 'flowing') into = out = 'flowing';
  // The lights' own effects: Prysm sends nothing; only the Lights stop is lit.
  if (path.kind === 'failed') {
    into = failedAt === 'output' ? 'still' : 'failed';
    out = failedAt === 'capture' ? 'still' : 'failed';
  }
  const flowing = path.kind === 'flowing' ? path.input : null;
  // The rate counts the frames Prysm takes and sends: it is Prysm's figure.
  // What keeps Prysm from keeping up, if anything: working, so orange, not
  // red. A failure shows no flowing path, so red always wins.
  const behind = flowing?.behind;
  const unnamed =
    behind !== undefined && behind !== 'processing' && behind !== 'output';
  let rateDescription =
    'frames per second Prysm processes and sends, over the last second';
  if (behind === 'processing') {
    rateDescription =
      'Prysm can’t keep up: processing each frame takes longer than the time between frames, so some are skipped. Frames per second Prysm processes and sends, over the last second';
  } else if (unnamed) {
    rateDescription =
      'Prysm can’t keep up. Frames per second Prysm processes and sends, over the last second';
  }
  const processed = flowing
    ? { text: frameRate(flowing.fps), about: rateDescription }
    : undefined;
  const total = edges ? ledCount(edges) : undefined;
  // How far behind the picture the lights are: the two links added, only
  // when the source says when it captured (otherwise the start is unknown).
  const parts = flowing?.latency;
  const lagMs =
    parts?.capture_ms === undefined
      ? undefined
      : parts.capture_ms + parts.output_ms;
  const lag =
    lagMs === undefined
      ? undefined
      : {
          text: `${millis(lagMs)} behind`,
          about: `The lights show each frame about ${millis(lagMs)} after it was captured. The lights’ own delay isn’t included.`,
        };
  return (
    <section
      className="setup-card"
      data-path={path.kind}
      aria-labelledby={headingId}
    >
      <h2 id={headingId} className="overline">
        Your setup
      </h2>
      <ol className="path">
        <Stop
          title="Video source"
          icon={<VideoIcon />}
          name={captureName}
          fact={
            path.kind === 'waiting'
              ? waitingFact[path.picture]
              : factLine(capture.facts, ['Rate', 'Menus'])
          }
          failed={failedAt === 'capture'}
        />
        <Link
          state={into}
          transport={capture.transport}
          numbers={delay(flowing?.latency?.capture_ms)}
          about={
            flowing?.latency?.capture_ms === undefined
              ? undefined
              : 'adds the time from the camera capturing a frame to Prysm receiving it, mean over the last second'
          }
        />
        <Stop
          title="Processing"
          icon={<PrysmIcon />}
          name="Prysm"
          fact={calibration ?? undefined}
          figure={processed}
          failed={path.kind === 'failed' && failedAt === null}
          behind={behind === 'processing' || unnamed}
        />
        <Link
          state={out}
          transport={output.transport}
          numbers={delay(flowing?.latency?.output_ms)}
          about={
            flowing?.latency
              ? `${
                  behind === 'output'
                    ? 'Prysm can’t keep up: sending to the lights takes longer than the time between frames, so some are skipped. '
                    : ''
                }adds the time inside Prysm from a frame received to its colours sent, mean over the last second; the lights’ own delay isn’t included`
              : undefined
          }
          behind={behind === 'output'}
        />
        <Stop
          title="Lights"
          icon={<LightsIcon />}
          name={outputName}
          fact={total === undefined ? factLine(output.facts) : `${total} LEDs`}
          figure={lag}
          instead
          failed={failedAt === 'output'}
        />
      </ol>
      {edges && preview ? (
        <PreviewDrawing
          edges={edges}
          caption={ledCaption(edges)}
          source={preview}
        />
      ) : null}
      {edges && !preview ? (
        <LedDrawing edges={edges} caption={ledCaption(edges)} />
      ) : null}
      {/* A row as in Settings, at the card's foot. */}
      <ul className="rows setup-change">
        <RowLink
          label="Change setup"
          value="Video source, lights and picture"
          onClick={openSettings}
        />
      </ul>
    </section>
  );
}
