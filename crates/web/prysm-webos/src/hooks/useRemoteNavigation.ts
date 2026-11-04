import {
  useEffect,
  useEffectEvent,
  useLayoutEffect,
  useRef,
  useState,
  type MouseEvent,
  type FocusEvent,
} from 'react';

export type Dialog =
  | 'receiver'
  | 'source'
  | 'compression'
  | 'autostart'
  | 'service'
  | 'diagnostics'
  | 'cleanup'
  | 'exit';

// Keep numeric key codes for older TV remotes.
export function arrowDirection(keyCode: number) {
  switch (keyCode) {
    case 37:
      return 'left';
    case 38:
      return 'up';
    case 39:
      return 'right';
    case 40:
      return 'down';
    default:
      return undefined;
  }
}

export function useRemoteNavigation(suspended = false) {
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const wasSuspended = useRef(false);
  const lastSetting = useRef('receiver');
  const returnFocus = useRef<HTMLElement | null>(null);
  const modal = useRef<HTMLDivElement>(null);
  function openDialog(next: Dialog) {
    if (!dialog)
      returnFocus.current =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
    setDialog(next);
  }
  function closeDialog() {
    setDialog(
      dialog === 'diagnostics' || dialog === 'cleanup' ? 'service' : null,
    );
  }
  useLayoutEffect(() => {
    if (dialog) {
      (
        modal.current?.querySelector<HTMLElement>('[aria-pressed="true"]') ??
        modal.current?.querySelector<HTMLElement>('input, button')
      )?.focus();
    } else {
      const previous = returnFocus.current;
      if (
        previous &&
        !(previous instanceof HTMLButtonElement && previous.disabled)
      )
        previous.focus();
    }
  }, [dialog]);
  useLayoutEffect(() => {
    if (wasSuspended.current && !suspended)
      document.getElementById('calibrate')?.focus();
    wasSuspended.current = suspended;
    if (!dialog && document.activeElement === document.body)
      document.getElementById('primary')?.focus();
  });

  const keydown = useEffectEvent((event: KeyboardEvent) => {
    if (suspended) return;
    const active =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : document.body;
    const key = event.keyCode;
    if (key === 461 || key === 27) {
      event.preventDefault();
      if (dialog) closeDialog();
      else openDialog('exit');
      return;
    }
    const controls = Array.from(
      modal.current?.querySelectorAll<HTMLElement>('button, input') ?? [],
    );
    if (dialog && key === 9) {
      event.preventDefault();
      controls[
        (controls.indexOf(active) +
          (event.shiftKey ? controls.length - 1 : 1)) %
          controls.length
      ]?.focus();
      return;
    }
    const direction = arrowDirection(key);
    if (!direction) return;
    if (
      active.tagName === 'INPUT' &&
      (direction === 'left' || direction === 'right')
    )
      return;
    event.preventDefault();
    if (dialog) {
      if (direction === 'up' || direction === 'down')
        controls[
          controls.indexOf(active) + (direction === 'down' ? 1 : -1)
        ]?.focus();
    } else {
      let target = active.getAttribute('data-' + direction);
      if (
        direction === 'right' &&
        (active.id === 'primary' || active.id === 'service-options')
      )
        target = document
          .getElementById(lastSetting.current)
          ?.matches(':enabled')
          ? lastSetting.current
          : 'calibrate';
      const control = target ? document.getElementById(target) : null;
      if (control instanceof HTMLButtonElement && !control.disabled)
        control.focus();
    }
  });
  useEffect(() => {
    const listener = (event: KeyboardEvent) => keydown(event);
    document.addEventListener('keydown', listener);
    return () => document.removeEventListener('keydown', listener);
  }, []);

  function onMouseOver(event: MouseEvent<HTMLDivElement>) {
    const control =
      event.target instanceof Element ? event.target.closest('button') : null;
    if (
      control &&
      !control.disabled &&
      (!dialog || modal.current?.contains(control))
    )
      control.focus();
  }
  function onFocus(event: FocusEvent<HTMLDivElement>) {
    if (event.target.classList.contains('setting'))
      lastSetting.current = event.target.id;
  }
  return {
    dialog,
    modal,
    openDialog,
    closeDialog,
    dismiss: () => setDialog(null),
    onMouseOver,
    onFocus,
  };
}
