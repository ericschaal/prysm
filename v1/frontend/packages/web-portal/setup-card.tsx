import { useEffect, useId, useState, type ReactNode } from 'react';
import type { DiscoveryClient } from '@prysm/application-client';
import type { AdapterFact, AdapterSection, LedEdges } from './adapters';

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
 * The device's own name, such as the capture card's, when discovery finds
 * the saved device: looked up once per saved section, quietly; the kind's
 * name until then or when it is not found.
 */
function useDeviceName(discovery: DiscoveryClient | null, part: SetupPart) {
  const [found, setFound] = useState<{ key: string; name: string } | null>(
    null,
  );
  const key = JSON.stringify(part.saved);
  const { section } = part;
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
        if (match && !abort.signal.aborted)
          setFound({ key, name: match.label });
      })
      .catch(() => {
        // Not found or not available: the kind's name stays.
      });
    return () => abort.abort();
  }, [discovery, section, key]);
  return found?.key === key ? found.name : part.label;
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
        <text className="led-map-total" x="160" y="97" textAnchor="middle">
          {total} LEDs
        </text>
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

/** One section: its role, its name, and its facts set as a small spec. */
function Part({
  title,
  icon,
  part,
  discovery,
}: {
  title: string;
  icon: ReactNode;
  part: SetupPart;
  discovery: DiscoveryClient | null;
}) {
  const name = useDeviceName(discovery, part);
  return (
    <li>
      <span className="setup-icon">{icon}</span>
      <div className="setup-text">
        <span className="setup-name">{title}</span>
        <strong>{name}</strong>
        {part.facts.length ? (
          <dl className="setup-facts">
            {part.facts.map((fact) => (
              <div key={fact.label}>
                <dt>{fact.label}</dt>
                <dd>{fact.value}</dd>
              </div>
            ))}
          </dl>
        ) : null}
      </div>
    </li>
  );
}

/**
 * Your setup: where the picture comes from and where the colours go, by
 * name with a few facts, and the LEDs around the TV. Only what Prysm's
 * settings (and discovery, for names) say; the forms are in Settings.
 */
export function SetupCard({
  capture,
  output,
  edges,
  discovery,
  openSettings,
}: {
  capture: SetupPart;
  output: SetupPart;
  edges?: LedEdges;
  discovery: DiscoveryClient | null;
  openSettings: () => void;
}) {
  const headingId = useId();
  return (
    <section className="setup-card" aria-labelledby={headingId}>
      <h2 id={headingId} className="overline">
        Your setup
      </h2>
      <ol className="setup-flow">
        <Part
          title="Video source"
          icon={<SourceIcon />}
          part={capture}
          discovery={discovery}
        />
        <Part
          title="Lights"
          icon={<LightsIcon />}
          part={output}
          discovery={discovery}
        />
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
