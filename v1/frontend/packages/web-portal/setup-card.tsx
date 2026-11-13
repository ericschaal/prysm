import { useEffect, useId, useState, type ReactNode } from 'react';
import type { DiscoveryClient } from '@prysm/application-client';
import type {
  AdapterFact,
  AdapterSection,
  LedEdges,
  Transport,
} from './adapters';

/** A configured section for Home: what it is, and a few facts about it. */
export interface SetupPart {
  section: 'capture' | 'output';
  saved: AdapterSection;
  /** The kind's name, used until (or unless) the device's own is found. */
  label: string;
  facts: AdapterFact[];
}

/** Whether every field the candidate fills has the saved value. */
function picks(fields: Record<string, unknown>, saved: AdapterSection) {
  const values = saved as unknown as Record<string, unknown>;
  const keys = Object.keys(fields);
  return keys.length > 0 && keys.every((key) => fields[key] === values[key]);
}

/**
 * Discovered device names, kept in this browser: one per section and kind,
 * with the candidate fields that identify the device (a WLED address, a
 * capture device path). Shown at once on the next visit; never sent to Prysm.
 */
export const deviceNamesKey = 'prysm.deviceNames';

interface KnownName {
  fields: Record<string, unknown>;
  name: string;
}

function readNames(): Record<string, KnownName> {
  try {
    const stored = window.localStorage.getItem(deviceNamesKey);
    const names: unknown = stored ? JSON.parse(stored) : {};
    return names && typeof names === 'object'
      ? (names as Record<string, KnownName>)
      : {};
  } catch {
    return {};
  }
}

function writeNames(names: Record<string, KnownName>) {
  try {
    window.localStorage.setItem(deviceNamesKey, JSON.stringify(names));
  } catch {
    // Storage unavailable: names are found again next time.
  }
}

/** The kept name for `saved`, dropping one that belongs to another device. */
function knownName(slot: string, saved: AdapterSection) {
  const names = readNames();
  const known = names[slot];
  if (!known) return null;
  if (
    known.fields &&
    typeof known.name === 'string' &&
    picks(known.fields, saved)
  )
    return known.name;
  delete names[slot];
  writeNames(names);
  return null;
}

/**
 * The device's own name, such as the capture card's: the one kept from the
 * last visit at once, refreshed from discovery in the background (and kept
 * again when it changed); the kind's name only when none is known.
 */
function useDeviceName(discovery: DiscoveryClient | null, part: SetupPart) {
  const key = JSON.stringify(part.saved);
  const { section } = part;
  const slot = `${section}:${part.saved.kind}`;
  const [found, setFound] = useState<{ key: string; name: string | null }>(
    () => ({ key, name: knownName(slot, part.saved) }),
  );
  if (found.key !== key) setFound({ key, name: knownName(slot, part.saved) });
  useEffect(() => {
    if (!discovery) return;
    // By value: a new object with the same values is the same device.
    const saved = JSON.parse(key) as AdapterSection;
    const abort = new AbortController();
    discovery
      .candidates(section, saved.kind, abort.signal)
      .then((candidates) => {
        const match = candidates.find((candidate) =>
          picks(candidate.fields, saved),
        );
        if (!match || abort.signal.aborted) return;
        setFound({ key, name: match.label });
        writeNames({
          ...readNames(),
          [`${section}:${saved.kind}`]: {
            fields: match.fields,
            name: match.label,
          },
        });
      })
      .catch(() => {
        // Not found or not available: the kept (or kind's) name stays.
      });
    return () => abort.abort();
  }, [discovery, section, key]);
  return (found.key === key ? found.name : null) ?? part.label;
}

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
      /** Frames arriving, and leaving for the lights, per second. */
      fps: number;
      sentFps: number;
      /** Mean ms from a frame received to its colours sent (inside Prysm). */
      latencyMs?: number;
    }
  | { kind: 'waiting' }
  | { kind: 'effects' }
  | { kind: 'failed'; at: 'capture' | 'output' | null };

function VideoIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <rect x="3" y="4.5" width="18" height="12" rx="2" />
      <path d="M8.5 20h7M12 16.5V20" />
    </svg>
  );
}

/** Prysm's mark: the prism the light passes through. */
function PrysmIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M12 3.5 21 19.5H3Z" />
      <path className="ray" d="M12 3.5l2.6 16M3 19.5 13.6 11l7.4 8.5" />
    </svg>
  );
}

function LightsIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M9.5 18h5M10.5 21h3M8 14.5a6 6 0 1 1 8 0c-.9.8-1.3 1.7-1.3 2.5H9.3c0-.8-.4-1.7-1.3-2.5Z" />
    </svg>
  );
}

/** One stop on the path: its role (for screen readers), name and one fact. */
function Stop({
  title,
  icon,
  name,
  fact,
  failed,
}: {
  title: string;
  icon: ReactNode;
  name: string;
  fact?: string;
  failed: boolean;
}) {
  return (
    <li className="path-stop" data-failed={failed}>
      <span className="path-icon">{icon}</span>
      <span className="path-text">
        <span className="visually-hidden">{title}: </span>
        {/* Never broken at a hyphen ("WLED-Gledopto"). */}
        <strong className="path-name">{name.split('-').join('\u2011')}</strong>
        {/* Always a line, so the stops stay aligned when one has no fact. */}
        <span className="path-fact">{fact ?? '\u00a0'}</span>
      </span>
    </li>
  );
}

/** A link between two stops: a hairline, its light, and what it carries. */
function Link({
  state,
  transport,
  numbers,
  about,
}: {
  state: string;
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
  const rate =
    path.kind === 'flowing' ? `${Math.round(path.fps)} fps` : undefined;
  // What leaves for the lights: the rate, and the span inside Prysm.
  const flowing = path.kind === 'flowing' ? path : null;
  const sent = flowing ? `${Math.round(flowing.sentFps)} fps` : undefined;
  const latency =
    flowing?.latencyMs === undefined
      ? undefined
      : `${Math.round(flowing.latencyMs)} ms`;
  const total = edges
    ? edges.top + edges.right + edges.bottom + edges.left
    : undefined;
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
        <Link state={into} transport={transports.capture} numbers={rate} />
        <Stop
          title="Processing"
          icon={<PrysmIcon />}
          name="Prysm"
          // Only a positive fact: the active profile. Nothing when none.
          fact={profile ? `${profile} profile` : undefined}
          failed={path.kind === 'failed' && failedAt === null}
        />
        <Link
          state={out}
          transport={transports.output}
          numbers={sent && (latency ? `${sent} · ${latency}` : sent)}
          about="rate of frames sent; time measured inside Prysm, from a frame received to its colours sent, mean over the last second"
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
              failed={failedAt === 'output'}
            />
          )}
        </Named>
      </ol>
      {edges ? <LedMap edges={edges} /> : null}
      <div className="setup-links">
        <button type="button" className="quiet" onClick={openSettings}>
          Change setup
        </button>
      </div>
    </section>
  );
}
