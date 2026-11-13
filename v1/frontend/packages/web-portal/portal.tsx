import type { ApplicationRuntime } from '@prysm/application-state';
import type {
  ApplicationSnapshot,
  DiscoveryClient,
  PreviewClient,
} from '@prysm/application-client';
import { useEffect, useRef, useState } from 'react';
import {
  useApplication,
  useApplicationDispatch,
} from '@prysm/application-react';
import { adapterFor, type AdapterSection, type AdapterUis } from './adapters';
import { calibrateRoute, CalibrateWizard } from './calibrate';
import { DiscoveryContext } from './discovery-context';
import { LightingControls } from './lighting';
import { PreviewPanel } from './preview';
import { ProcessingPanel } from './processing';
import { SectionForm } from './section-form';
import { SetupWizard } from './setup';
import './portal.css';

export interface WebPortalProps<
  C extends AdapterSection,
  O extends AdapterSection,
> {
  application: ApplicationRuntime<C, O>;
  /** The capture kinds this host registers, in display order. */
  captures: AdapterUis<NoInfer<C>>;
  /** The output kinds this host registers, in display order. */
  outputs: AdapterUis<NoInfer<O>>;
  /** The live preview; without one the portal shows no Preview panel. */
  preview?: PreviewClient;
  /**
   * Finds devices to choose from in the capture and lights editors; without
   * one they offer only typed fields.
   */
  discovery?: DiscoveryClient;
}

/**
 * Whether the page shows the Calibrate page (`#/calibrate`), and a way to
 * open or leave it. The hash keeps the portal's own address and history.
 */
function useCalibrateRoute() {
  const [hash, setHash] = useState(() => window.location.hash);
  useEffect(() => {
    const sync = () => setHash(window.location.hash);
    window.addEventListener('hashchange', sync);
    return () => window.removeEventListener('hashchange', sync);
  }, []);
  function open(calibrate: boolean) {
    const next = calibrate ? calibrateRoute : '';
    setHash(next);
    window.location.hash = next;
  }
  return [hash === calibrateRoute, open] as const;
}

const sessionLabels = {
  stopped: 'stopped',
  running: 'running',
  native: 'device effects',
  faulted: 'faulted',
} as const;

/**
 * The browser portal: lighting modes, live picture controls and one form per
 * adapter section, or the guided setup while capture or lights are missing,
 * or the Calibrate page at `#/calibrate`.
 * The host chooses which capture and output kinds exist.
 */
export function WebPortal<C extends AdapterSection, O extends AdapterSection>({
  application,
  captures,
  outputs,
  preview,
  discovery,
}: WebPortalProps<C, O>) {
  const connection = useApplication(application);
  const dispatch = useApplicationDispatch<C, O>();
  // Keep showing the last state (and every draft) while reconnecting.
  const [last, setLast] = useState<ApplicationSnapshot<C, O> | null>(null);
  if (connection.snapshot && connection.snapshot !== last)
    setLast(connection.snapshot);
  const snapshot = connection.snapshot ?? last;
  const online = connection.status === 'connected' && !!connection.snapshot;
  // Setup opens by itself when the first state lacks capture or lights.
  const [setup, setSetup] = useState<'pending' | 'open' | 'closed'>('pending');
  if (setup === 'pending' && snapshot)
    setSetup(
      snapshot.settings.capture && snapshot.settings.output ? 'closed' : 'open',
    );
  /** How setup ended; focused when shown. */
  const [setupNotice, setSetupNotice] = useState('');
  const noticeRef = useRef<HTMLParagraphElement>(null);
  const [calibrating, openCalibrate] = useCalibrateRoute();
  useEffect(() => {
    if (setupNotice) noticeRef.current?.focus();
  }, [setupNotice]);

  let connectionLabel = 'Offline';
  if (online) connectionLabel = 'Connected';
  else if (connection.status === 'connecting') connectionLabel = 'Connecting…';
  const header = (
    <header className="portal-header">
      <span className="brand">
        prysm<span aria-hidden="true">.</span>
      </span>
      <span
        className={`connection ${online ? 'online' : 'offline'}`}
        role="status"
      >
        {connectionLabel}
      </span>
    </header>
  );
  if (!snapshot)
    return (
      <div className="portal">
        {header}
        <main id="content">
          <h1 className="visually-hidden">Prysm</h1>
          <section className="card">
            <h2>Connecting to Prysm</h2>
            <p>
              {connection.error?.message ?? 'Waiting for the current state…'}
            </p>
          </section>
        </main>
      </div>
    );

  const { settings, revision, session } = snapshot;
  const output = settings.output ?? null;
  const outputLighting = output
    ? (adapterFor(outputs, output.kind)?.lighting?.(output) ?? {})
    : null;
  const offline = !online ? (
    <p className="notice warning" role="alert">
      Connection lost
      {connection.error ? `: ${connection.error.message}` : ''}. Reconnecting;
      unsaved changes are kept.
    </p>
  ) : null;
  if (calibrating)
    return (
      <div className="portal">
        {header}
        <main id="content">
          <h1 className="visually-hidden">Prysm calibration</h1>
          {offline}
          <CalibrateWizard
            processing={settings.processing}
            revision={revision}
            target={snapshot.target}
            calibration={snapshot.calibration}
            disabled={!online}
            save={(expected, processing) =>
              dispatch(
                application.commands.configureProcessing(expected, processing),
              )
            }
            setCalibration={(calibration) =>
              dispatch(application.commands.setCalibration(calibration))
            }
            leave={() => openCalibrate(false)}
          />
        </main>
      </div>
    );
  if (setup === 'open')
    return (
      <div className="portal">
        {header}
        <main id="content">
          <h1 className="visually-hidden">Prysm setup</h1>
          {offline}
          <DiscoveryContext value={discovery ?? null}>
            <SetupWizard
              captures={captures}
              outputs={outputs}
              capture={settings.capture ?? null}
              output={output}
              revision={revision}
              disabled={!online}
              save={(expected, capture, value) =>
                dispatch(
                  application.commands.configureSetup(expected, capture, value),
                )
              }
              start={() =>
                dispatch(application.commands.setTarget('processed'))
              }
              close={(notice) => {
                setSetup('closed');
                setSetupNotice(notice);
              }}
            />
          </DiscoveryContext>
        </main>
      </div>
    );
  return (
    <div className="portal">
      {header}
      <main id="content">
        <h1 className="visually-hidden">Prysm</h1>
        {offline}
        {setupNotice ? (
          <p className="notice" role="status" tabIndex={-1} ref={noticeRef}>
            {setupNotice}
          </p>
        ) : null}
        <LightingControls
          target={snapshot.target}
          session={session}
          failure={snapshot.failure}
          outputIssue={snapshot.output_issue}
          input={snapshot.input}
          hasCapture={!!settings.capture}
          hasOutput={!!output}
          output={outputLighting}
          disabled={!online}
          setTarget={(target) =>
            dispatch(application.commands.setTarget(target))
          }
        />
        {preview ? (
          <PreviewPanel
            client={preview}
            online={online}
            target={snapshot.target}
            session={session}
          />
        ) : null}
        <ProcessingPanel
          processing={settings.processing}
          revision={revision}
          disabled={!online}
          running={session.state === 'running'}
          save={(expected, processing) =>
            dispatch(
              application.commands.configureProcessing(expected, processing),
            )
          }
        />
        <div className="setup-entry">
          <p>Match the lights to your TV with test patterns shown on the TV.</p>
          <button type="button" onClick={() => openCalibrate(true)}>
            Calibrate colours
          </button>
        </div>
        <div className="setup-entry">
          <p>
            {settings.capture && output
              ? 'Change capture and lights step by step.'
              : 'Prysm needs a video source and lights before screen sync can run.'}
          </p>
          <button
            type="button"
            onClick={() => {
              setSetupNotice('');
              setSetup('open');
            }}
          >
            {settings.capture && output ? 'Run setup again' : 'Run setup'}
          </button>
        </div>
        <DiscoveryContext value={discovery ?? null}>
          <SectionForm
            section="capture"
            title="Capture"
            description="Where the picture comes from. Saving restarts screen sync if it is running."
            adapters={captures}
            saved={settings.capture ?? null}
            revision={revision}
            disabled={!online}
            save={(expected, capture) =>
              dispatch(application.commands.configureCapture(expected, capture))
            }
          />
          <SectionForm
            section="output"
            title="Lights"
            description="Where the colours go. Saving restarts screen sync if it is running."
            adapters={outputs}
            saved={output}
            revision={revision}
            disabled={!online}
            save={(expected, value) =>
              dispatch(application.commands.configureOutput(expected, value))
            }
          />
        </DiscoveryContext>
        {snapshot.surface_failures.map((failure, index) => (
          // Failures have no identity; the list is replaced whole and rows hold no state.
          // eslint-disable-next-line @eslint-react/no-array-index-key
          <p key={index} className="notice warning">
            A Prysm surface stopped: {failure.message}. Lighting continues.
          </p>
        ))}
        <details className="card diagnostics">
          <summary>Technical details</summary>
          <dl>
            <div>
              <dt>Target</dt>
              <dd>{snapshot.target}</dd>
            </div>
            <div>
              <dt>Session</dt>
              <dd>
                {sessionLabels[session.state]}
                {session.state === 'faulted' ? ` (${session.phase})` : ''}
              </dd>
            </div>
            <div>
              <dt>Settings revision</dt>
              <dd>{revision}</dd>
            </div>
          </dl>
        </details>
      </main>
    </div>
  );
}
