import type { KeyboardEvent } from 'react';

const arrowSteps: Readonly<Record<string, number>> = {
  ArrowRight: 1,
  ArrowDown: 1,
  ArrowLeft: -1,
  ArrowUp: -1,
};

/**
 * Arrow keys move a radio group's choice, as in any radio group: from
 * `from`, one step along `values` (wrapping), offered to `choose`; when it
 * takes it, the focus follows to that radio, the group's child at the same
 * place.
 */
export function arrowChoice<T>(
  event: KeyboardEvent<HTMLElement>,
  values: readonly T[],
  from: T,
  choose: (next: T) => boolean,
) {
  const step = arrowSteps[event.key];
  if (!step) return;
  event.preventDefault();
  const index = (values.indexOf(from) + step + values.length) % values.length;
  const next = values[index];
  if (next === undefined || !choose(next)) return;
  const radios = event.currentTarget.parentElement?.children;
  (radios?.[index] as HTMLElement | undefined)?.focus();
}
