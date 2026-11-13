import { refusedValue } from './form';
import type { ComponentType } from 'react';

/** An adapter settings section: `{ kind, ...fields }`. */
export interface AdapterSection {
  kind: string;
}

/**
 * A controlled editor for one adapter section. It renders fields from
 * `./fields`, which take their disabled state and errors from the form.
 */
export interface EditorProps<S> {
  value: S;
  /** Keep fields the editor does not show: `onChange({ ...value, path })`. */
  onChange: (next: S) => void;
}

/** What the lighting controls offer for a configured output. */
export interface OutputLighting {
  /** Label of the device's own effects (`device_native`); absent if it has none. */
  effects?: string;
  /** A link to the device's own interface. */
  link?: { label: string; href: string };
}

/** LEDs on each edge of the screen, seen from the front. */
export interface LedEdges {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

/**
 * How a link of the signal path carries its data: the lights' protocol, or
 * a capture stream's encoding. A closed set, named by the portal.
 */
export type Transport = 'local' | 'file' | 'built_in' | 'h264' | 'raw' | 'ddp';

/** One fact about a configured section, for Home: `Picture` `640 × 360`. */
export interface AdapterFact {
  label: string;
  value: string;
}

/** A picture size for facts: `640 × 360`. */
export const pictureSize = (width: number, height: number) =>
  `${width}\u00a0×\u00a0${height}`;

/** One registered adapter kind: how the portal names, creates and edits it. */
export interface AdapterUi<S extends AdapterSection> {
  label: string;
  /** A new section of this kind, before the user fills it in. */
  create: () => S;
  Editor: ComponentType<EditorProps<S>>;
  /** Tidy what the user typed just before saving, e.g. trim an address. */
  normalize?: (value: S) => S;
  /** One line for the setup review, e.g. the device path and picture size. */
  summary?: (value: S) => string;
  /** The few facts Home shows about the saved section, as typed data. */
  facts?: (saved: S) => AdapterFact[];
  /** Outputs only: lighting actions for the saved section. */
  lighting?: (saved: S) => OutputLighting;
  /**
   * How the saved section's data travels: the stream from a capture, the
   * colours to an output. Absent for a local device (nothing to name).
   */
  transport?: (saved: S) => Transport;
  /** Outputs only: the LEDs around the screen, for the drawing on Home. */
  edges?: (saved: S) => LedEdges;
  /**
   * What a field says when Prysm refuses its value, by field path
   * (`output.address`): how to fill it in. Others say `refusedValue`.
   */
  problems?: Readonly<Record<string, string>>;
}

/** What the field at `path` says when Prysm refused its value. */
export function refusal(
  adapter: { problems?: AdapterUi<AdapterSection>['problems'] } | undefined,
  path: string,
) {
  return adapter?.problems?.[path] ?? refusedValue;
}

/**
 * The kinds a host registers for one section, keyed by kind and in display
 * order. The mapping is exhaustive over the host's generated union.
 */
export type AdapterUis<S extends AdapterSection> = {
  [K in S['kind']]: AdapterUi<Extract<S, { kind: K }>>;
};

export function adapterFor<S extends AdapterSection>(
  adapters: AdapterUis<S>,
  kind: string,
): AdapterUi<S> | undefined {
  // Indexing a mapped type by a runtime kind loses the per-kind narrowing.
  return (adapters as unknown as Record<string, AdapterUi<S> | undefined>)[
    kind
  ];
}

/** A picture size and rate for summaries: `640 × 360 at 30 fps`. */
export function describePicture(width: number, height: number, fps: number) {
  return `${width}\u00a0×\u00a0${height} at ${fps}\u00a0fps`;
}

/** A new section of `kind`, as its panel creates it. */
export function createSection<S extends AdapterSection>(
  adapters: AdapterUis<S>,
  kind: string,
): S {
  const adapter = adapterFor(adapters, kind);
  if (!adapter) throw new Error(`No panel for kind ${kind}`);
  return adapter.create();
}

export function kindsOf<S extends AdapterSection>(adapters: AdapterUis<S>) {
  return Object.keys(adapters) as S['kind'][];
}
