/**
 * Remote control: arrows move focus (never values), OK activates, Back closes
 * one dialog level and then asks before closing the app.
 *
 * The screen has two columns (`data-column`); Up/Down move within a column,
 * Left/Right move within a `data-row` group and otherwise to the other column,
 * returning to the control last used there. In a dialog Up/Down (and Tab)
 * move through its controls, and past the first or last one scroll its
 * `data-scroll` text; Left/Right stay with a text field's caret.
 */
import {
  useEffect,
  useEffectEvent,
  useLayoutEffect,
  useRef,
  useState,
  type FocusEvent,
  type MouseEvent,
  type RefObject,
} from 'react';
import type { DialogId } from './dialogs';

/** Key codes, numeric for older TV remotes. */
export const keys = {
  left: 37,
  up: 38,
  right: 39,
  down: 40,
  enter: 13,
  escape: 27,
  back: 461,
  tab: 9,
} as const;

type Direction = 'left' | 'up' | 'right' | 'down';

function direction(event: KeyboardEvent): Direction | undefined {
  switch (event.keyCode || event.key) {
    case keys.left:
    case 'ArrowLeft':
      return 'left';
    case keys.up:
    case 'ArrowUp':
      return 'up';
    case keys.right:
    case 'ArrowRight':
      return 'right';
    case keys.down:
    case 'ArrowDown':
      return 'down';
    default:
      return undefined;
  }
}
const isBack = (event: KeyboardEvent) =>
  event.keyCode === keys.back ||
  event.keyCode === keys.escape ||
  event.key === 'Escape' ||
  event.key === 'GoBack';

/** The focused element; no `instanceof`, which needs DOM globals. */
const activeElement = () => {
  const active = document.activeElement as HTMLElement | null;
  return active && active !== document.body ? active : null;
};

/** Enabled buttons and fields in document order (jsdom groups selector lists). */
const focusables = (scope: ParentNode | null | undefined) =>
  Array.from(scope?.querySelectorAll<HTMLElement>('*') ?? []).filter(
    (element) =>
      (element.tagName === 'BUTTON' || element.tagName === 'INPUT') &&
      !(element as HTMLButtonElement).disabled &&
      !element.closest('[hidden]'),
  );

const usable = (
  element: HTMLElement | null | undefined,
): element is HTMLElement =>
  !!element &&
  element.isConnected &&
  !(element as HTMLButtonElement).disabled &&
  !element.closest('[hidden]');

/** A row group counts as one stop; entering it picks its chosen control. */
function rowStop(row: Element): HTMLElement | undefined {
  const controls = focusables(row);
  return (
    controls.find(
      (element) => element.getAttribute('aria-pressed') === 'true',
    ) ?? controls[0]
  );
}

/** The first stop in a scope: its first control, or a row's chosen one. */
function firstStop(scope: ParentNode | null | undefined) {
  const first = focusables(scope)[0];
  const row = first?.closest('[data-row]');
  return row ? rowStop(row) : first;
}

/**
 * Where focus rests when the app places it: the first stop, unless OK there
 * would change something (a switch); then the first control that only opens.
 */
function restingStop(scope: ParentNode | null | undefined) {
  const first = firstStop(scope);
  if (first?.getAttribute('role') !== 'switch') return first;
  return (
    focusables(scope).find((el) => el.getAttribute('role') !== 'switch') ??
    first
  );
}

export function useRemoteNavigation({
  root,
  onExitRequest,
  settled,
}: {
  /** The page outside dialogs. */
  root: RefObject<HTMLElement | null>;
  /** Back outside any dialog. */
  onExitRequest: () => void;
  /**
   * The page shows its controls as they will be; until then the app places
   * no focus, so it does not land on the one control enabled meanwhile.
   */
  settled: boolean;
}) {
  const [path, setPath] = useState<
    { dialog: DialogId; opener: HTMLElement | null }[]
  >([]);
  const dialog = path[path.length - 1]?.dialog ?? null;
  const modal = useRef<HTMLDivElement>(null);
  const lastInColumn = useRef<Record<string, HTMLElement>>({});

  function openDialog(dialog: DialogId) {
    const opener = activeElement();
    setPath((path) => [...path, { dialog, opener }]);
  }
  const closeDialog = () => setPath((path) => path.slice(0, -1));
  const dismiss = () => setPath([]);

  // A shorter path returns to the control that opened its first removed dialog.
  // Dialog content remounts, so find that control by id; the page keeps its node.
  const previous = useRef(path);
  useLayoutEffect(() => {
    const left = previous.current[path.length];
    previous.current = path;
    if (dialog) {
      const controls = focusables(modal.current);
      (
        controls.find(
          (element) => left?.opener?.id && element.id === left.opener.id,
        ) ??
        controls.find(
          (element) => element.getAttribute('aria-pressed') === 'true',
        ) ??
        controls[0]
      )?.focus();
    } else if (left) {
      (usable(left.opener) ? left.opener : firstStop(root.current))?.focus();
    }
  }, [path, dialog, root]);
  // A status update can disable the focused control; never leave focus on the
  // page. Until the user moves, focus goes to the primary button, or without
  // one to the resting stop (the chosen lighting mode), once it can.
  const automatic = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    if (dialog || !settled) return;
    const active = activeElement();
    const preferred =
      root.current?.querySelector<HTMLElement>('#primary:not(:disabled)') ??
      restingStop(root.current);
    if (
      active &&
      usable(active) &&
      !(active === automatic.current && preferred && preferred !== active)
    )
      return;
    if (preferred && preferred !== active) {
      // Set first: a control focused for the user is not one they chose.
      automatic.current = preferred;
      preferred.focus();
    }
  });

  const onKey = useEffectEvent((event: KeyboardEvent) => {
    if (event.defaultPrevented) return;
    automatic.current = null;
    if (isBack(event)) {
      event.preventDefault();
      if (dialog) closeDialog();
      else onExitRequest();
      return;
    }
    const active = activeElement();
    if (dialog && (event.keyCode === keys.tab || event.key === 'Tab')) {
      event.preventDefault();
      const controls = focusables(modal.current);
      const index = active ? controls.indexOf(active) : -1;
      const step = event.shiftKey ? controls.length - 1 : 1;
      controls[(index + step) % controls.length]?.focus();
      return;
    }
    const to = direction(event);
    if (!to || (!dialog && !root.current)) return;
    const horizontal = to === 'left' || to === 'right';
    if (horizontal && active?.tagName === 'INPUT') return;
    event.preventDefault();
    const column = active?.closest('[data-column]');
    const scope = dialog ? modal.current : column;
    if (!scope) {
      if (!dialog) firstStop(root.current)?.focus();
      return;
    }
    const row = active?.closest('[data-row]');
    const step = to === 'right' || to === 'down' ? 1 : -1;
    if (horizontal) {
      if (active && row) {
        const items = focusables(row);
        const next = items[items.indexOf(active) + step];
        if (next) return next.focus();
      }
      if (dialog || !root.current || !column) return;
      const columns = Array.from(
        root.current.querySelectorAll('[data-column]'),
      );
      const other = columns[columns.indexOf(column) + step];
      if (!other) return;
      const remembered =
        lastInColumn.current[other.getAttribute('data-column') ?? ''];
      (usable(remembered) && other.contains(remembered)
        ? remembered
        : firstStop(other)
      )?.focus();
      return;
    }
    // Up/Down leaves the whole row and enters the next at its chosen control.
    const controls = focusables(scope);
    let index = active ? controls.indexOf(active) : -1;
    // A page row still counts as a stop if its focused button was just disabled.
    if (!dialog && row && index < 0)
      index = controls.findIndex((element) => row.contains(element));
    let next = index + step;
    let target = controls[next];
    while (target && row?.contains(target)) {
      next += step;
      target = controls[next];
    }
    if (target) {
      const group = target.closest('[data-row]');
      (group ? rowStop(group) : target)?.focus();
    } else if (dialog) {
      const text = scope.querySelector('[data-scroll]');
      if (text) text.scrollTop += (step * text.clientHeight * 2) / 3;
    }
  });
  useEffect(() => {
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  return {
    dialog,
    modal,
    openDialog,
    closeDialog,
    dismiss,
    /** Magic Remote pointer: hovering a control focuses it. */
    onMouseOver: (event: MouseEvent<HTMLElement>) => {
      const target = event.target as HTMLElement;
      const control =
        typeof target.closest === 'function'
          ? target.closest<HTMLElement>('button, input')
          : null;
      if (
        usable(control) &&
        (!dialog || modal.current?.contains(control)) &&
        document.activeElement !== control &&
        // Do not pull focus out of a field the user is typing in.
        document.activeElement?.tagName !== 'INPUT'
      )
        control.focus();
    },
    onFocus: (event: FocusEvent<HTMLElement>) => {
      const column = event.target.closest('[data-column]');
      const name = column?.getAttribute('data-column');
      if (name && event.target !== automatic.current)
        lastInColumn.current[name] = event.target;
    },
  };
}
