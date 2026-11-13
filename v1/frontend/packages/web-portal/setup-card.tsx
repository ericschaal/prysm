import { useId, type ReactNode } from 'react';
import type { DiscoveryClient } from '@prysm/application-client';
import type { AdapterFact, LedEdges, Transport } from './adapters';
import { useDeviceName, type SetupPart } from './device-name';
import { LightsIcon, PrysmIcon, VideoIcon } from './path-icons';
import { RowLink } from './rows';

const sides = ['top', 'right', 'bottom', 'left'] as const;

/**
 * The TV seen from the front on its stand, with the LED band around it:
 * each edge's run of light and, a little further out, its count.
 */
function LedMap({ edges }: { edges: LedEdges }) {
  const blurId = useId();
  const total = sides.reduce((sum, edge) => sum + edges[edge], 0);
  // Screen 200 × 112 at (60, 34); the band 9 out; the counts 22 out (the
  // bottom one past the stand).
  const band = {
    top: [66, 25, 254, 25],
    right: [269, 40, 269, 140],
    bottom: [254, 155, 66, 155],
    left: [51, 140, 51, 40],
  } as const;
  const count = {
    top: [160, 13],
    right: [289, 94],
    bottom: [160, 190],
    left: [31, 94],
  } as const;
  return (
    <figure className="led-map">
      <svg viewBox="0 0 320 200" aria-hidden="true" focusable="false">
        <defs>
          {/* The light the band throws on the wall: soft, not an outline. */}
          <filter
            id={blurId}
            filterUnits="userSpaceOnUse"
            x="0"
            y="0"
            width="320"
            height="200"
          >
            <feGaussianBlur stdDeviation="5" />
          </filter>
        </defs>
        <g className="led-map-band">
          {sides.map((edge) => {
            const [x1, y1, x2, y2] = band[edge];
            return (
              <g
                key={edge}
                className={`led-map-edge edge-${edge}${edges[edge] ? '' : ' empty'}`}
              >
                <line
                  className="halo"
                  filter={`url(#${blurId})`}
                  x1={x1}
                  y1={y1}
                  x2={x2}
                  y2={y2}
                />
                <line x1={x1} y1={y1} x2={x2} y2={y2} />
              </g>
            );
          })}
        </g>
        <path className="led-map-stand" d="M160 160v10M138 172h44" />
        <rect
          className="led-map-screen"
          x="60"
          y="34"
          width="200"
          height="112"
          rx="7"
        />
        {sides.map((edge) => (
          <text
            key={edge}
            className="led-map-count"
            x={count[edge][0]}
            y={count[edge][1] + 4}
            textAnchor="middle"
          >
            {edges[edge]}
          </text>
        ))}
      </svg>
      <figcaption className="visually-hidden">
        Seen from the front: {edges.top} LEDs along the top, {edges.right} on
        the right, {edges.bottom} along the bottom and {edges.left} on the left,{' '}
        {total} in all.
      </figcaption>
    </figure>
  );
}

/**
 * What the path is doing, from the state: frames flowing through Prysm,
 * screen sync on without a picture, the lights' own effects (Prysm not in
 * the path), stopped by a failure (at the source, at the lights, or
 * somewhere on the way), or off.
 */
export type PathState =
  | { kind: 'off' }
  | {
      kind: 'flowing';
      /** Frames arriving per second (each is sent on before the next). */
      fps: number;
      /**
       * Mean ms per frame on each link: capture to received (only when the
       * source stamps capture times), received to colours sent.
       */
      latency?: { captureMs?: number; outputMs: number };
      /** The step that keeps Prysm from keeping up, while one does. */
      behind?: 'processing' | 'output';
    }
  | { kind: 'waiting' }
  | { kind: 'effects' }
  | { kind: 'failed'; at: 'capture' | 'output' | null };

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
  /** A measured figure after the fact; the fact gives way to it, never the reverse. */
  figure?: { text: string; about: string };
  /**
   * The figure takes the fact's place on the line while it shows; the fact
   * stays in the stop's tooltip and screen-reader text.
   */
  instead?: boolean;
  failed: boolean;
  /** Working, but this stop keeps Prysm from keeping up (shown in orange). */
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
          {short.split('-').join('\u2011')}
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

/** What a link adds to the lag: "+88 ms", "+<1 ms". */
const delay = (ms: number | undefined) =>
  ms === undefined ? undefined : `+${millis(ms)}`;

/** A rate to whole frames, with the tenth only when it matters: `29.9 fps`. */
function frameRate(fps: number) {
  const whole = Math.round(fps);
  return `${Math.abs(fps - whole) < 0.05 ? whole : fps.toFixed(1)} fps`;
}

/** A link between two stops: a hairline, its light, and what it carries. */
function Link({
  state,
  transport,
  numbers,
  about,
  behind = false,
}: {
  state: string;
  /** Working, but this link keeps Prysm from keeping up (shown in orange). */
  behind?: boolean;
  /** Above the hairline: how the link carries its data. */
  transport?: Transport;
  /** Below it: the live numbers, while frames flow. */
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

/** A section's name, found by discovery when it can be. */
function Named({
  part,
  discovery,
  children,
}: {
  part: SetupPart;
  discovery: DiscoveryClient | null;
  children: (name: string) => ReactNode;
}) {
  return children(useDeviceName(discovery, part));
}

/** The facts as one quiet line: `640 × 480 · 30 fps`. */
const factLine = (facts: AdapterFact[], skip: string[] = []) =>
  facts
    .filter((fact) => !skip.includes(fact.label))
    .map((fact) => fact.value)
    .join(' · ') || undefined;

/**
 * Your setup, as the path the light takes: the video source, Prysm and the
 * lights, joined by two hairlines that carry what is measured (the picture
 * rate arriving) and light up while frames flow. Only what Prysm's state and
 * settings say (and discovery, for names); the forms are in Settings.
 */
export function SetupCard({
  capture,
  output,
  edges,
  transports,
  discovery,
  path,
  profile,
  openSettings,
}: {
  capture: SetupPart;
  output: SetupPart;
  edges?: LedEdges;
  /** How each link carries its data, when the adapter names it. */
  transports: { capture?: Transport; output?: Transport };
  discovery: DiscoveryClient | null;
  path: PathState;
  /** The active colour profile's name, or `null` for no correction. */
  profile: string | null;
  openSettings: () => void;
}) {
  const headingId = useId();
  const failedAt = path.kind === 'failed' ? path.at : undefined;
  // Which links are lit, still or broken.
  let into = 'still';
  let out = 'still';
  if (path.kind === 'flowing') into = out = 'flowing';
  // The lights' own effects: Prysm sends nothing; only the Lights stop is lit.
  if (path.kind === 'failed') {
    into = failedAt === 'output' ? 'still' : 'failed';
    out = failedAt === 'capture' ? 'still' : 'failed';
  }
  // The rate once, on the video stop; each link only its own delay.
  const flowing = path.kind === 'flowing' ? path : null;
  // The rate counts the frames Prysm takes and sends: it is Prysm's figure.
  // What keeps Prysm from keeping up, if anything: working, so orange, not
  // red. A failure shows no flowing path, so red always wins.
  const behind = flowing?.behind;
  const processed = flowing
    ? {
        text: frameRate(flowing.fps),
        about:
          behind === 'processing'
            ? 'Prysm can’t keep up: processing each frame takes longer than the time between frames, so some are skipped. Frames per second Prysm processes and sends, over the last second'
            : 'frames per second Prysm processes and sends, over the last second',
      }
    : undefined;
  const total = edges
    ? edges.top + edges.right + edges.bottom + edges.left
    : undefined;
  // How far behind the picture the lights are: the two links added, only
  // when the source says when it captured (otherwise the start is unknown).
  const parts = flowing?.latency;
  const lagMs =
    parts?.captureMs === undefined
      ? undefined
      : parts.captureMs + parts.outputMs;
  const lag =
    lagMs === undefined
      ? undefined
      : {
          text: `${millis(lagMs)} behind`,
          about: `The lights show each frame about ${millis(lagMs)} after it was captured. The lights’ own delay is not included.`,
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
        <Named part={capture} discovery={discovery}>
          {(name) => (
            <Stop
              title="Video source"
              icon={<VideoIcon />}
              name={name}
              // Waiting replaces the fact line; it never adds one.
              fact={
                path.kind === 'waiting'
                  ? 'Waiting for picture'
                  : factLine(capture.facts, ['Rate', 'Menus'])
              }
              failed={failedAt === 'capture'}
            />
          )}
        </Named>
        <Link
          state={into}
          transport={transports.capture}
          numbers={delay(flowing?.latency?.captureMs)}
          about={
            flowing?.latency?.captureMs === undefined
              ? undefined
              : 'adds the time from the camera capturing a frame to Prysm receiving it, mean over the last second'
          }
        />
        <Stop
          title="Processing"
          icon={<PrysmIcon />}
          name="Prysm"
          // Only a positive fact: the active profile. Nothing when none.
          // The profile by its name: the fps figure beside it must not cut it.
          fact={profile ?? undefined}
          figure={processed}
          failed={path.kind === 'failed' && failedAt === null}
          behind={behind === 'processing'}
        />
        <Link
          state={out}
          transport={transports.output}
          numbers={delay(flowing?.latency?.outputMs)}
          about={
            flowing?.latency
              ? `${
                  behind === 'output'
                    ? 'Prysm can’t keep up: sending to the lights takes longer than the time between frames, so some are skipped. '
                    : ''
                }adds the time inside Prysm from a frame received to its colours sent, mean over the last second; the lights’ own delay is not included`
              : undefined
          }
          behind={behind === 'output'}
        />
        <Named part={output} discovery={discovery}>
          {(name) => (
            <Stop
              title="Lights"
              icon={<LightsIcon />}
              name={name}
              fact={
                total === undefined ? factLine(output.facts) : `${total} LEDs`
              }
              figure={lag}
              instead
              failed={failedAt === 'output'}
            />
          )}
        </Named>
      </ol>
      {edges ? <LedMap edges={edges} /> : null}
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
