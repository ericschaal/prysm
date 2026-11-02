import { useEffect, useEffectEvent, useLayoutEffect, useRef, useState, type MouseEvent, type FocusEvent } from 'react';

export type Dialog = 'receiver' | 'source' | 'compression' | 'autostart' | 'service' | 'diagnostics' | 'cleanup' | 'exit';

export function useRemoteNavigation() {
    const [dialog, setDialog] = useState<Dialog | null>(null);
    const lastSetting = useRef('receiver');
    const returnFocus = useRef<HTMLElement | null>(null);
    const modal = useRef<HTMLDivElement>(null);
    function openDialog(next: Dialog) {
        if (!dialog) returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        setDialog(next);
    }
    function closeDialog() {
        setDialog(dialog === 'diagnostics' || dialog === 'cleanup' ? 'service' : null);
    }
    useLayoutEffect(() => {
        if (dialog) {
            (modal.current?.querySelector<HTMLElement>('[aria-pressed="true"]') ?? modal.current?.querySelector<HTMLElement>('input, button'))?.focus();
        } else {
            const previous = returnFocus.current;
            if (previous && !(previous instanceof HTMLButtonElement && previous.disabled)) previous.focus();
        }
    }, [dialog]);
    useLayoutEffect(() => {
        if (!dialog && document.activeElement === document.body) document.getElementById('primary')?.focus();
    });

    const keydown = useEffectEvent((event: KeyboardEvent) => {
        const active = document.activeElement instanceof HTMLElement ? document.activeElement : document.body;
        const key = event.keyCode;
        if (key === 461 || key === 27) {
            event.preventDefault();
            if (dialog) closeDialog(); else openDialog('exit');
            return;
        }
        const controls = Array.from(modal.current?.querySelectorAll<HTMLElement>('button, input') ?? []);
        if (dialog && key === 9) {
            event.preventDefault();
            controls[(controls.indexOf(active) + (event.shiftKey ? controls.length - 1 : 1)) % controls.length]?.focus();
            return;
        }
        if ([37, 38, 39, 40].indexOf(key) < 0) return;
        if (active.tagName === 'INPUT' && (key === 37 || key === 39)) return;
        event.preventDefault();
        if (dialog) {
            if (key === 38 || key === 40) controls[controls.indexOf(active) + (key === 40 ? 1 : -1)]?.focus();
        } else {
            const direction = ({ 37: 'left', 38: 'up', 39: 'right', 40: 'down' } as Record<number, string>)[key];
            let target = active.getAttribute('data-' + direction);
            if (key === 39 && (active.id === 'primary' || active.id === 'service-options')) target = lastSetting.current;
            const control = target ? document.getElementById(target) : null;
            if (control instanceof HTMLButtonElement && !control.disabled) control.focus();
        }
    });
    useEffect(() => {
        const listener = (event: KeyboardEvent) => keydown(event);
        document.addEventListener('keydown', listener);
        return () => document.removeEventListener('keydown', listener);
    }, []);

    function onMouseOver(event: MouseEvent<HTMLDivElement>) {
        const control = event.target instanceof Element ? event.target.closest('button') : null;
        if (control && !control.disabled && (!dialog || modal.current?.contains(control))) control.focus();
    }
    function onFocus(event: FocusEvent<HTMLDivElement>) {
        if (event.target.classList.contains('setting')) lastSetting.current = event.target.id;
    }
    return { dialog, modal, openDialog, closeDialog, dismiss: () => setDialog(null), onMouseOver, onFocus };
}
