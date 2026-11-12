import type { ApplicationRuntime } from '@prysm/application-state';
import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { LightingTarget } from '@prysm/application-client';
import {
  useApplication,
  useApplicationDispatch,
} from '@prysm/application-react';
import type { TvHostClient, TvHostStatus } from '@prysm/webos-platform';

export interface TvUiProps<C, O> {
  application: ApplicationRuntime<C, O>;
  platform: TvHostClient;
  portalUrl?: string;
  onExit: () => void;
}

/** Always packaged on the TV. It imports no browser portal or transport implementation. */
export function TvUi<C, O>({
  application,
  platform,
  portalUrl,
  onExit,
}: TvUiProps<C, O>) {
  const connection = useApplication(application);
  const dispatch = useApplicationDispatch<C, O>();
  const rootRef = useRef<HTMLElement>(null);
  const mountedRef = useRef(false);
  const [host, setHost] = useState<TvHostStatus | null>(null);
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    mountedRef.current = true;
    let disposed = false;
    void platform.status().then(
      (value) => {
        if (!disposed) setHost(value);
      },
      (error: unknown) => {
        if (!disposed) setNotice(String(error));
      },
    );
    rootRef.current
      ?.querySelector<HTMLButtonElement>('button:not(:disabled)')
      ?.focus();
    return () => {
      disposed = true;
      mountedRef.current = false;
    };
  }, [platform]);

  async function action(work: () => Promise<void>) {
    setBusy(true);
    setNotice('');
    try {
      await work();
    } catch (error) {
      if (mountedRef.current)
        setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      if (mountedRef.current) setBusy(false);
    }
  }
  const target = (value: LightingTarget) =>
    action(async () => {
      await dispatch(application.commands.setTarget(value));
    });
  function navigate(event: KeyboardEvent<HTMLElement>) {
    if (
      event.key === 'Escape' ||
      event.key === 'GoBack' ||
      event.keyCode === 461
    ) {
      event.preventDefault();
      onExit();
      return;
    }
    if (
      !['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(event.key)
    )
      return;
    const controls = Array.from(
      rootRef.current?.querySelectorAll<HTMLElement>(
        'button:not(:disabled), a[href]',
      ) ?? [],
    );
    if (!controls.length) return;
    event.preventDefault();
    const current = controls.indexOf(document.activeElement as HTMLElement);
    const next = event.key === 'ArrowUp' || event.key === 'ArrowLeft' ? -1 : 1;
    controls[(current + next + controls.length) % controls.length]?.focus();
  }
  const unavailable = busy || connection.status !== 'connected';
  let serviceStatus = 'checking';
  if (host) serviceStatus = host.service_ready ? 'ready' : 'unavailable';
  let captureStatus = host?.capture_available ? 'available' : 'unavailable';
  if (host?.forwarding) captureStatus = 'forwarding frames';
  return (
    // eslint-disable-next-line jsx-a11y-x/no-noninteractive-element-interactions -- TV remote keys are delegated from focusable descendant controls.
    <main ref={rootRef} onKeyDown={navigate}>
      <h1>Prysm on TV</h1>
      <p>Local service: {serviceStatus}</p>
      <p>Local capture: {captureStatus}</p>
      <p>
        Application: {connection.status}; requested lighting:{' '}
        {connection.snapshot?.target ?? 'unknown'}
      </p>
      <div>
        <button disabled={unavailable} onClick={() => void target('processed')}>
          Turn lighting on
        </button>
        <button disabled={unavailable} onClick={() => void target('off')}>
          Turn lighting off
        </button>
        <button
          disabled={busy}
          onClick={() =>
            void action(async () => {
              await platform.command('inspect_service');
              setHost(await platform.status());
            })
          }
        >
          Inspect TV service
        </button>
        <button
          disabled={busy}
          onClick={() =>
            void action(async () => {
              await platform.command('repair_service');
              setHost(await platform.status());
            })
          }
        >
          Repair TV service
        </button>
      </div>
      {portalUrl ? (
        <p>
          Use the web portal for capture and output settings:{' '}
          <a href={portalUrl}>{portalUrl}</a>
        </p>
      ) : null}
      <p role="status">{notice || connection.error}</p>
      <p>Arrow keys: move · OK: choose · Back: leave the UI</p>
    </main>
  );
}
