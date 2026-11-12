/**
 * Remote control: arrows move focus (never values), OK activates, Back closes
 * one dialog level and then asks before closing the app.
 *
 * The screen has two columns (`data-column`); Up/Down move within a column,
 * Left/Right move within a `data-row` group and otherwise to the other column,
 * returning to the control last used there. In a dialog Up/Down (and Tab)
 * move through its controls; Left/Right stay with a text field's caret.
 */
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type FocusEvent,
  type MouseEvent,
} from 'react';

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

const usable = (element: HTMLElement | null | undefined): element is HTMLElement =>
  !!element &&
  element.isConnected &&
  !(element as HTMLButtonElement).disabled &&
  !element.closest('[hidden]');

/** A row group counts as one stop; entering it picks its chosen control. */
function rowStop(row: Element): HTMLElement | undefined {
  const controls = focusables(row);
  return (
    controls.find((element) => element.getAttribute('aria-pressed') === 'true') ??
    controls[0]
  );
}

export function useRemoteNavigation<D extends string>({
  parents,
  onExitRequest,
}: {
  /** Back from these dialogs returns to their parent dialog. */
  parents: Partial<Record<D, D>>;
  /** Back outside any dialog. */
  onExitRequest: () => void;
}) {
  const [dialog, setDialog] = useState<D | null>(null);
  const root = useRef<HTMLElement>(null);
  const modal = useRef<HTMLDivElement>(null);
  const returnFocus = useRef<HTMLElement | null>(null);
  const lastInColumn = useRef<Record<string, HTMLElement>>({});
  const opened = useRef<D | null>(null);

  function openDialog(next: D) {
    if (!dialog) returnFocus.current = activeElement();
    setDialog(next);
  }
  function closeDialog() {
    setDialog((current) => (current ? (parents[current] ?? null) : null));
  }
  const dismiss = () => setDialog(null);

  // Focus the chosen option (or the first control) when a dialog opens, and the
  // opener when the last one closes.
  useLayoutEffect(() => {
    if (opened.current === dialog) return;
    opened.current = dialog;
    if (dialog) {
      const controls = focusables(modal.current);
      (
        controls.find((element) => element.getAttribute('aria-pressed') === 'true') ??
        controls[0]
      )?.focus();
    } else if (usable(returnFocus.current)) returnFocus.current.focus();
    else focusables(root.current)[0]?.focus();
  });
  // A status update can disable the focused control; never leave focus on the
  // page. Until the user moves, focus goes to the primary button once it can.
  const automatic = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    if (dialog) return;
    const active = activeElement();
    const primary = root.current?.querySelector<HTMLElement>('#primary:not(:disabled)');
    if (active && usable(active) && !(active === automatic.current && primary)) return;
    const next = primary ?? focusables(root.current)[0];
    if (next && next !== active) {
      next.focus();
      automatic.current = next;
    }
  });

  const handler = useRef<(event: KeyboardEvent) => void>(() => {});
  handler.current = (event) => {
    if (event.defaultPrevented) return;
    automatic.current = null;
    if (isBack(event)) {
      event.preventDefault();
      if (dialog) closeDialog();
      else onExitRequest();
      return;
    }
    const active = activeElement();
    if (dialog) {
      const controls = focusables(modal.current);
      const index = active ? controls.indexOf(active) : -1;
      if (event.keyCode === keys.tab || event.key === 'Tab') {
        event.preventDefault();
        const step = event.shiftKey ? controls.length - 1 : 1;
        controls[(index + step) % controls.length]?.focus();
        return;
      }
      const to = direction(event);
      if (!to) return;
      if (active?.tagName === 'INPUT' && (to === 'left' || to === 'right')) return;
      event.preventDefault();
      const row = active?.closest('[data-row]');
      if (row && (to === 'left' || to === 'right')) {
        const items = focusables(row);
        items[items.indexOf(active!) + (to === 'right' ? 1 : -1)]?.focus();
        return;
      }
      if (to === 'up' || to === 'down') {
        // Skip the rest of a row group when leaving it vertically.
        const step = to === 'down' ? 1 : -1;
        let next = index + step;
        while (row && controls[next] && row.contains(controls[next]!)) next += step;
        const target = controls[next];
        const targetRow = target?.closest('[data-row]');
        (targetRow && targetRow !== row ? rowStop(targetRow) : target)?.focus();
      }
      return;
    }
    const to = direction(event);
    if (!to || !root.current) return;
    if (active?.tagName === 'INPUT' && (to === 'left' || to === 'right')) return;
    event.preventDefault();
    const column = active?.closest('[data-column]');
    if (!column) {
      focusables(root.current)[0]?.focus();
      return;
    }
    const row = active!.closest('[data-row]');
    if (to === 'left' || to === 'right') {
      if (row) {
        const items = focusables(row);
        const next = items[items.indexOf(active!) + (to === 'right' ? 1 : -1)];
        if (next) {
          next.focus();
          return;
        }
      }
      const columns = Array.from(root.current.querySelectorAll('[data-column]'));
      const other = columns[columns.indexOf(column) + (to === 'right' ? 1 : -1)];
      if (!other) return;
      const remembered = lastInColumn.current[other.getAttribute('data-column') ?? ''];
      (usable(remembered) && other.contains(remembered)
        ? remembered
        : focusables(other)[0]
      )?.focus();
      return;
    }
    // Up/Down: the next stop in this column, a row group being one stop.
    const stops: HTMLElement[] = [];
    const rows = new Set<Element>();
    for (const element of focusables(column)) {
      const group = element.closest('[data-row]');
      if (!group) stops.push(element);
      else if (!rows.has(group)) {
        rows.add(group);
        const stop = rowStop(group);
        if (stop) stops.push(stop);
      }
    }
    const here = row ? stops.findIndex((stop) => row.contains(stop)) : stops.indexOf(active!);
    stops[here + (to === 'down' ? 1 : -1)]?.focus();
  };
  useEffect(() => {
    const listener = (event: KeyboardEvent) => handler.current(event);
    document.addEventListener('keydown', listener);
    return () => document.removeEventListener('keydown', listener);
  }, []);

  return {
    dialog,
    root,
    modal,
    openDialog,
    closeDialog,
    dismiss,
    /** Magic Remote pointer: hovering a control focuses it. */
    onMouseOver(event: MouseEvent<HTMLElement>) {
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
    onFocus(event: FocusEvent<HTMLElement>) {
      const column = event.target.closest('[data-column]');
      const name = column?.getAttribute('data-column');
      if (name) lastInColumn.current[name] = event.target;
    },
  };
}
