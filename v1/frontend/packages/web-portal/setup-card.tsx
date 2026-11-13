import { useId } from 'react';
import type { LedEdges } from './adapters';

/** A configured section in words: its kind and the adapter's summary. */
export interface SetupPart {
  label: string;
  summary?: string;
}

/** Where each edge's LEDs are drawn, and its count. */
const edgeLines: Record<
  keyof LedEdges,
  { line: [number, number, number, number]; count: [number, number] }
> = {
  top: { line: [70, 22, 250, 22], count: [160, 11] },
  right: { line: [284, 52, 284, 142], count: [306, 101] },
  bottom: { line: [250, 172, 70, 172], count: [160, 192] },
  left: { line: [36, 142, 36, 52], count: [14, 101] },
};

/** The screen seen from the front with the LEDs on each edge. */
function LedMap({ edges }: { edges: LedEdges }) {
  const sides = Object.keys(edgeLines) as (keyof LedEdges)[];
  const total = sides.reduce((sum, edge) => sum + edges[edge], 0);
  return (
    <figure className="led-map">
      <svg viewBox="0 0 320 200" aria-hidden="true" focusable="false">
        <rect
          className="led-map-screen"
          x="56"
          y="38"
          width="208"
          height="118"
          rx="8"
        />
        <text className="led-map-total" x="160" y="98" textAnchor="middle">
          {total} LEDs
        </text>
        <text className="led-map-note" x="160" y="118" textAnchor="middle">
          seen from the front
        </text>
        {sides.map((edge) => {
          const { line, count } = edgeLines[edge];
          return (
            <g
              key={edge}
              className={`led-map-edge edge-${edge}${edges[edge] ? '' : ' empty'}`}
            >
              <line x1={line[0]} y1={line[1]} x2={line[2]} y2={line[3]} />
              <text x={count[0]} y={count[1] + 4} textAnchor="middle">
                {edges[edge]}
              </text>
            </g>
          );
        })}
      </svg>
      <figcaption className="visually-hidden">
        {total} LEDs: {edges.top} along the top, {edges.right} on the right,{' '}
        {edges.bottom} along the bottom and {edges.left} on the left.
      </figcaption>
    </figure>
  );
}

function SourceIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <rect x="3" y="4.5" width="18" height="12" rx="2" />
      <path d="M8.5 20h7M12 16.5V20" />
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

/**
 * Your setup: where the picture comes from, where the colours go, and the
 * LEDs around the screen when the output says how many. Only what Prysm's
 * settings say; the forms are in Settings.
 */
export function SetupCard({
  capture,
  output,
  edges,
  link,
  openSettings,
}: {
  capture: SetupPart;
  output: SetupPart;
  edges?: LedEdges;
  link?: { label: string; href: string };
  openSettings: () => void;
}) {
  const headingId = useId();
  const parts = [
    { name: 'Video source', part: capture, icon: <SourceIcon /> },
    { name: 'Lights', part: output, icon: <LightsIcon /> },
  ];
  return (
    <section className="setup-card" aria-labelledby={headingId}>
      <p className="overline" aria-hidden="true">
        Your setup
      </p>
      <h2 id={headingId}>Screen &amp; lights</h2>
      <ol className="setup-flow">
        {parts.map(({ name, part, icon }) => (
          <li key={name}>
            <span className="setup-icon">{icon}</span>
            <span className="setup-text">
              <span className="setup-name">{name}</span>
              <strong>{part.label}</strong>
              {part.summary ? (
                <span className="setup-summary">{part.summary}</span>
              ) : null}
            </span>
          </li>
        ))}
      </ol>
      {edges ? <LedMap edges={edges} /> : null}
      <div className="setup-links">
        <button type="button" className="quiet" onClick={openSettings}>
          Change setup
        </button>
        {link ? (
          <a href={link.href} target="_blank" rel="noopener noreferrer">
            {link.label}
            <span className="visually-hidden"> (opens in a new tab)</span>
          </a>
        ) : null}
      </div>
    </section>
  );
}
