import type { ComponentType } from 'react';
import { ApplicationError, type Failure } from '@prysm/application-client';

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

/** One registered adapter kind: how the portal names, creates and edits it. */
export interface AdapterUi<S extends AdapterSection> {
  label: string;
  /** A new section of this kind, before the user fills it in. */
  create: () => S;
  Editor: ComponentType<EditorProps<S>>;
  /** Tidy what the user typed just before saving, e.g. trim an address. */
  normalize?: (value: S) => S;
  /** Outputs only: lighting actions for the saved section. */
  lighting?: (saved: S) => OutputLighting;
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
  return (adapters as unknown as Record<string, AdapterUi<S> | undefined>)[kind];
}

export function kindsOf<S extends AdapterSection>(adapters: AdapterUis<S>) {
  return Object.keys(adapters) as S['kind'][];
}

/** Structural equality, ignoring object key order (the server may reorder). */
export function same(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || !a || !b) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const keys = Object.keys(left).filter((key) => left[key] !== undefined);
  if (keys.length !== Object.keys(right).filter((key) => right[key] !== undefined).length)
    return false;
  return keys.every((key) => same(left[key], right[key]));
}

export function failureOf(error: unknown): Failure {
  return error instanceof ApplicationError
    ? error.failure
    : {
        kind: 'internal',
        message: error instanceof Error ? error.message : String(error),
      };
}
