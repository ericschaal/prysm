import { requestId } from '@prysm/application-state';
import type { ApplicationRuntime } from '@prysm/application-state';
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from 'react';
import type { CalibrationPresentation } from '@prysm/application-client';
import {
  useApplication,
  useApplicationDispatch,
} from '@prysm/application-react';
import type { TvHostClient, PatternPresenter } from '@prysm/webos-platform';
import type { TvHostStatus } from '@prysm/contracts/tv-host';
import { createPresentationController } from './presentation';

export interface TvUiProps<C, O> {
  application: ApplicationRuntime<C, O>;
  platform: TvHostClient;
  presenter: PatternPresenter;
  portalUrl?: string;
  onExit: () => void;
  onCleanupFailure: (error: unknown) => void;
}

/** Always packaged on the TV. It imports no browser portal or transport implementation. */
export function TvUi<C, O>({
  application,
  platform,
  presenter,
  portalUrl,
  onExit,
  onCleanupFailure,
}: TvUiProps<C, O>) {
  const connection = useApplication(application);
  const dispatch = useApplicationDispatch<C, O>();
  const rootRef = useRef<HTMLElement>(null);
  const [host, setHost] = useState<TvHostStatus | null>(null);
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);
  const [calibration, setCalibration] =
    useState<CalibrationPresentation | null>(null);
  const mountedRef = useRef(false);
  const cleanupFailureRef = useRef(onCleanupFailure);
  useEffect(() => {
    cleanupFailureRef.current = onCleanupFailure;
  }, [onCleanupFailure]);
  const presentation = useMemo(
    () =>
      createPresentationController(application.client.calibration, presenter),
    [application, presenter],
  );

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
    const hide = () => {
      if (!document.hidden) return;
      setCalibration(null);
      void presentation
        .stop()
        .catch((error: unknown) => cleanupFailureRef.current(error));
    };
    const leave = () => {
      void presentation
        .stop()
        .catch((error: unknown) => cleanupFailureRef.current(error));
    };
    window.addEventListener('pagehide', leave);
    document.addEventListener('visibilitychange', hide);
    return () => {
      disposed = true;
      mountedRef.current = false;
      window.removeEventListener('pagehide', leave);
      document.removeEventListener('visibilitychange', hide);
      void presentation
        .stop()
        .catch((error: unknown) => cleanupFailureRef.current(error));
    };
  }, [platform, presentation]);

  async function action(work: () => Promise<void>) {
    setBusy(true);
    setNotice('');
    try {
      await work();
    } catch (error) {
      if (mountedRef.current) setNotice(String(error));
    } finally {
      if (mountedRef.current) setBusy(false);
    }
  }
  function navigate(event: KeyboardEvent<HTMLElement>) {
    if (
      event.key === 'Escape' ||
      event.key === 'GoBack' ||
      event.keyCode === 461
    ) {
      event.preventDefault();
      if (calibration || busy) {
        setCalibration(null);
        void action(() => presentation.stop());
      } else onExit();
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
        <button
          disabled={unavailable}
          onClick={() =>
            void action(async () => {
              const reply = await dispatch(
                application.commands.setTarget('processed'),
              );
              setNotice(`Accepted operation ${reply.operation.request}.`);
            })
          }
        >
          Turn lighting on
        </button>
        <button
          disabled={unavailable}
          onClick={() =>
            void action(async () => {
              const reply = await dispatch(
                application.commands.setTarget('off'),
              );
              setNotice(`Accepted operation ${reply.operation.request}.`);
            })
          }
        >
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
        <button
          disabled={unavailable || !!calibration}
          onClick={() =>
            void action(async () => {
              const value = await presentation.start(
                requestId(application.store.getState()),
              );
              if (mountedRef.current) setCalibration(value);
            })
          }
        >
          Start calibration
        </button>
        {calibration ? (
          <button
            onClick={() =>
              void action(async () => {
                setCalibration(null);
                await presentation.stop();
              })
            }
          >
            Close calibration
          </button>
        ) : null}
      </div>
      {portalUrl ? (
        <p>
          Use the web portal for capture settings, preview and full calibration:{' '}
          <a href={portalUrl}>{portalUrl}</a>
        </p>
      ) : null}
      {calibration ? (
        <section aria-label="Calibration presentation">
          <h2>Calibration presenter</h2>
          <p>Pattern: {calibration.pattern.id}</p>
          <p>Pairing: {calibration.pairing}</p>
        </section>
      ) : null}
      <p role="status">{notice || connection.error}</p>
      <p>
        Arrow keys: move · OK: choose · Back: close calibration or leave the UI
      </p>
    </main>
  );
}
