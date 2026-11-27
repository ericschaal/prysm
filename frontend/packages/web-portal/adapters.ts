import {
  isObject,
  type AdapterSide,
  type Fields,
} from '@prysm/application-client';
import type { CaptureSettings } from '@prysm/contracts/application';
import { refusedValue } from './form';
import type { ComponentType } from 'react';

/** An adapter settings section: `{ kind, ...fields }`. */
export type AdapterSection = CaptureSettings;

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
  /** Label of the device's own effects (the target `effects`); absent if it has none. */
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

/** All the LEDs around the screen. */
export const ledCount = (edges: LedEdges) =>
  edges.top + edges.right + edges.bottom + edges.left;

/**
 * `The lights have 120 LEDs, Prysm sends 96.`: while the lights' own LED
 * count is known and differs from the configured edges' total (a dark tail,
 * or the last edge cut short); `null` otherwise.
 */
export function ledCountsDiffer(
  device: number | undefined,
  sent: number | undefined,
) {
  return device === undefined || sent === undefined || device === sent
    ? null
    : `The lights have ${device} LEDs, Prysm sends ${sent}.`;
}

/** A device name never broken at a hyphen ("WLED-Gledopto"). */
export const unbroken = (name: string) => name.split('-').join('\u2011');

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

/** A capture's size and rate; an omitted one means the Rust default. */
export interface Picture {
  width?: number;
  height?: number;
  fps?: number;
}

export const savedPicture = (value: Picture, defaults: Required<Picture>) => ({
  width: value.width ?? defaults.width,
  height: value.height ?? defaults.height,
  fps: value.fps ?? defaults.fps,
});

/** A capture's size and rate as Home's facts: `Picture`, then `Rate`. */
export function pictureFacts(
  value: Picture,
  defaults: Required<Picture>,
): AdapterFact[] {
  const { width, height, fps } = savedPicture(value, defaults);
  return [
    { label: 'Picture', value: pictureSize(width, height) },
    { label: 'Rate', value: `${fps} fps` },
  ];
}

/** A capture's size and rate for summaries: `640 × 360 at 30 fps`. */
export function describeSavedPicture(
  value: Picture,
  defaults: Required<Picture>,
) {
  const { width, height, fps } = savedPicture(value, defaults);
  return describePicture(width, height, fps);
}

/** The fields of a section type `S` that its sections may leave out. */
export type OptionalField<S> = {
  [K in keyof S]-?: Partial<Pick<S, K>> extends Pick<S, K> ? K : never;
}[keyof S];

/** One registered adapter kind: how the portal names, creates and edits it. */
export interface AdapterUi<S extends AdapterSection> {
  label: string;
  /**
   * For development and tests only (a video file, no lights): offered with
   * developer options on, or while the saved setup is of this kind.
   */
  developer?: boolean;
  /** A new section of this kind, before the user fills it in. */
  create: () => S;
  /** Defaults for fields implied by a newly created section but left out of it. */
  readDefaults?: Partial<S>;
  /** The optional fields and the types of their defaults. */
  optional: Required<Pick<S, OptionalField<S>>>;
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
  /** Setup only: what to do on the device, in one line after the intro. */
  setup?: string;
  /**
   * What to check while the saved section fails, in one line, shown with
   * the failure in its editor.
   */
  trouble?: string;
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
export type AdapterUis<S extends AdapterSection = AdapterSection> = {
  [K in S['kind']]: AdapterUi<Extract<S, { kind: K }>>;
};

export function adapterFor<A extends Record<string, unknown>, K extends string>(
  adapters: A,
  kind: K,
): A[K] | undefined {
  return Object.prototype.hasOwnProperty.call(adapters, kind)
    ? adapters[kind]
    : undefined;
}

export function setupPart(
  section: AdapterSide,
  adapters: AdapterUis,
  saved: AdapterSection,
) {
  const panel = adapterFor(adapters, saved.kind);
  return {
    section,
    saved,
    label: panel?.label ?? saved.kind,
    facts: panel?.facts?.(saved) ?? [],
    edges: panel?.edges?.(saved),
    transport: panel?.transport?.(saved),
    lighting: panel?.lighting?.(saved),
  };
}

export type SetupPart = ReturnType<typeof setupPart>;

/** A picture size and rate for summaries: `640 × 360 at 30 fps`. */
export function describePicture(width: number, height: number, fps: number) {
  return `${width}\u00a0×\u00a0${height} at ${fps}\u00a0fps`;
}

/** A new section of `kind`, as its panel creates it. */
export function createSection<S extends AdapterSection>(
  adapters: AdapterUis<S>,
  kind: S['kind'] = kindsOf(adapters)[0] ?? '',
): S {
  const adapter = adapterFor(adapters, kind);
  if (!adapter) throw new Error(`No panel for kind ${kind}`);
  return adapter.create();
}

/** Project values for an editor while the observed settings retain the original. */
export function readSection<S extends AdapterSection>(
  adapters: AdapterUis<S>,
  saved: S,
): AdapterSection {
  const adapter = adapterFor(adapters, saved.kind);
  if (!adapter) return saved;
  return {
    ...editorFields(
      saved,
      { ...adapter.create(), ...adapter.readDefaults, ...adapter.optional },
      adapter.optional,
    ),
    kind: saved.kind,
  };
}

/** Keep values of the right shape, including those outside the adapter's limits. */
function editorFields(
  saved: Fields,
  defaults: Fields,
  optional: Fields,
): Fields {
  const fields = { ...saved };
  for (const [key, fallback] of Object.entries(defaults)) {
    const value = saved[key];
    if (isObject(fallback)) {
      // Nested settings such as WLED's LED counts default each omitted field.
      fields[key] = editorFields(
        isObject(value) ? value : {},
        fallback,
        fallback,
      );
    } else if (
      typeof value !== typeof fallback ||
      (typeof value === 'number' && !Number.isFinite(value))
    ) {
      if (Object.prototype.hasOwnProperty.call(optional, key))
        delete fields[key];
      else fields[key] = fallback;
    }
  }
  return fields;
}

/** Registered kind names in display order. */
export function kindsOf<K extends string>(adapters: Record<K, unknown>): K[] {
  return Object.keys(adapters) as K[];
}

/**
 * The kinds to offer: all of them with developer options on, else those not
 * meant for development, plus any in `keep` (what is saved or being edited).
 */
export function offeredKinds<K extends string>(
  adapters: Readonly<Record<K, { developer?: boolean }>>,
  developer: boolean,
  keep: readonly string[],
) {
  return kindsOf(adapters).filter(
    (kind) => developer || !adapters[kind]?.developer || keep.includes(kind),
  );
}
