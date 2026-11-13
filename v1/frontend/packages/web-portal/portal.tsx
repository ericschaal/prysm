import type { ApplicationRuntime } from '@prysm/application-state';
import type {
  ApplicationSnapshot,
  DiscoveryClient,
  PreviewClient,
} from '@prysm/application-client';
import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import {
  useApplication,
  useApplicationDispatch,
} from '@prysm/application-react';
import { adapterFor, type AdapterSection, type AdapterUis } from './adapters';
import { calibrateRoute, CalibrateWizard } from './calibrate';
import { DiscoveryContext } from './discovery-context';
import { LightingControls } from './lighting';
import { PreviewPanel } from './preview';
import { ProcessingPanel, QuickPicture } from './processing';
import { SectionForm } from './section-form';
import { SetupWizard } from './setup';
import { ThemeChoice } from './theme-choice';
import './tokens.css';
import './portal.css';
import './flows.css';

export interface WebPortalProps<
  C extends AdapterSection,
  O extends AdapterSection,
> {
  application: ApplicationRuntime<C, O>;
  /** The capture kinds this host registers, in display order. */
  captures: AdapterUis<NoInfer<C>>;
  /** The output kinds this host registers, in display order. */
  outputs: AdapterUis<NoInfer<O>>;
  /** The live preview; without one the portal shows no preview. */
  preview?: PreviewClient;
  /**
   * Finds devices to choose from in the capture and lights editors; without
   * one they offer only typed fields.
   */
  discovery?: DiscoveryClient;
}

/** The hash route of the Settings page. */
export const settingsRoute = '#/settings';

type Route = 'home' | 'settings' | 'calibrate';

/**
 * Which page the hash names (Home, `#/settings`, `#/calibrate`) and a way to
 * go to another. The hash keeps the portal's own address and history.
 */
function useRoute() {
  const [hash, setHash] = useState(() => window.location.hash);
  useEffect(() => {
    const sync = () => setHash(window.location.hash);
    window.addEventListener('hashchange', sync);
    return () => window.removeEventListener('hashchange', sync);
  }, []);
  function go(next: string) {
    setHash(next);
    window.location.hash = next;
  }
  let route: Route = 'home';
  if (hash === calibrateRoute) route = 'calibrate';
  else if (hash === settingsRoute) route = 'settings';
  return [route, go] as const;
}

const sessionLabels = {
  stopped: 'stopped',
  running: 'running',
  native: 'device effects',
  faulted: 'faulted',
} as const;

function SettingsIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
      <circle cx="15" cy="7" r="2" />
      <circle cx="9" cy="17" r="2" />
    </svg>
  );
}

function ChevronIcon() {
  return (
    <svg
      className="chevron"
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M9 5.5 15.5 12 9 18.5" />
    </svg>
  );
}

/** A row in Settings that opens a flow: what it does, and the button. */
function Entry({
  description,
  label,
  onClick,
}: {
  description: string;
  label: string;
  onClick: () => void;
}) {
  const id = useId();
  return (
    <li className="entry">
      <button type="button" aria-describedby={id} onClick={onClick}>
        {label}
        <ChevronIcon />
      </button>
      {/* Drawn inside the row; the whole row is the button. */}
      <span id={id} className="entry-description">
        {description}
      </span>
    </li>
  );
}

/**
 * The browser portal. Home shows the lighting state, the main switch, the
 * live picture and brightness; Settings (`#/settings`) holds the picture
 * tuning, the capture and lights forms and the technical details; the
 * Calibrate page is at `#/calibrate`; the guided setup takes over while
 * capture or lights are missing. The host chooses which capture and output
 * kinds exist.
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
  const [route, go] = useRoute();
  /** Where Calibrate returns to: Settings when opened from there. */
  const [calibrateFrom, setCalibrateFrom] = useState('');
  const settingsHeadingRef = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (setupNotice) noticeRef.current?.focus();
  }, [setupNotice]);
  // Arriving in Settings: its heading, for screen readers and keyboards.
  useEffect(() => {
    if (route === 'settings')
      settingsHeadingRef.current?.focus({ preventScroll: true });
  }, [route]);

  let connectionLabel = 'Offline';
  if (online) connectionLabel = 'Connected';
  else if (connection.status === 'connecting') connectionLabel = 'Connecting…';
  else if (snapshot) connectionLabel = 'Reconnecting…';
  const header = (
    <header className="portal-header">
      <span className="brand">
        <svg
          className="brand-mark"
          viewBox="0 0 32 32"
          aria-hidden="true"
          focusable="false"
        >
          <path className="brand-prism" d="M16 4 28.5 26.5h-25Z" />
          <path
            className="brand-ray"
            d="M16 4l3.6 22.5M3.5 26.5 18.2 15 28.5 26.5"
          />
        </svg>
        prysm<span aria-hidden="true">.</span>
      </span>
      <span className="header-end">
        <span
          className={`connection ${online ? 'online' : 'offline'}`}
          role="status"
        >
          {connectionLabel}
        </span>
        {route === 'home' && snapshot && setup !== 'open' ? (
          <button
            type="button"
            className="icon-button"
            aria-label="Settings"
            onClick={() => go(settingsRoute)}
          >
            <SettingsIcon />
          </button>
        ) : null}
      </span>
    </header>
  );
  const page = (className: string, title: string, content: ReactNode) => (
    <div className="portal">
      {header}
      <main id="content" className={className}>
        <h1 className="visually-hidden">{title}</h1>
        {content}
      </main>
    </div>
  );
  if (!snapshot)
    return page(
      'portal-flow',
      'Prysm',
      <section className="connecting">
        <h2>Connecting to Prysm</h2>
        <p>{connection.error?.message ?? 'Waiting for the current state…'}</p>
      </section>,
    );

  const { settings, revision, session } = snapshot;
  const output = settings.output ?? null;
  const configured = !!settings.capture && !!output;
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
  const openSetup = () => {
    setSetupNotice('');
    setSetup('open');
  };
  const notices = (
    <>
      {offline}
      {setupNotice ? (
        <p className="notice" role="status" tabIndex={-1} ref={noticeRef}>
          {setupNotice}
        </p>
      ) : null}
    </>
  );
  const saveProcessing = (
    expected: number,
    processing: (typeof settings)['processing'],
  ) => dispatch(application.commands.configureProcessing(expected, processing));

  if (route === 'calibrate')
    return page(
      'portal-flow portal-flow-narrow',
      'Prysm calibration',
      <>
        {offline}
        <CalibrateWizard
          processing={settings.processing}
          revision={revision}
          target={snapshot.target}
          calibration={snapshot.calibration}
          disabled={!online}
          save={saveProcessing}
          setCalibration={(calibration) =>
            dispatch(application.commands.setCalibration(calibration))
          }
          leave={() => {
            go(calibrateFrom);
            setCalibrateFrom('');
          }}
        />
      </>,
    );
  if (setup === 'open')
    return page(
      'portal-flow',
      'Prysm setup',
      <>
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
            start={() => dispatch(application.commands.setTarget('processed'))}
            close={(notice) => {
              setSetup('closed');
              setSetupNotice(notice);
            }}
          />
        </DiscoveryContext>
      </>,
    );

  if (route === 'settings')
    return page(
      'portal-page settings',
      'Prysm settings',
      <>
        <div className="page-heading">
          <button type="button" className="back quiet" onClick={() => go('')}>
            <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
              <path d="M15 5.5 8.5 12l6.5 6.5" />
            </svg>
            Home
          </button>
          <h2 ref={settingsHeadingRef} tabIndex={-1}>
            Settings
          </h2>
        </div>
        {notices}
        <ProcessingPanel
          processing={settings.processing}
          revision={revision}
          disabled={!online}
          running={session.state === 'running'}
          save={saveProcessing}
        />
        <section className="settings-section" aria-labelledby="setup-heading">
          <h2 id="setup-heading">Your setup</h2>
          <ul className="entries">
            <Entry
              label="Calibrate colours"
              description="Match the lights to your TV with test patterns."
              onClick={() => {
                setCalibrateFrom(settingsRoute);
                go(calibrateRoute);
              }}
            />
            <Entry
              label={configured ? 'Run setup again' : 'Run setup'}
              description={
                configured
                  ? 'Change the video source and lights step by step.'
                  : 'Prysm needs a video source and lights before screen sync can run.'
              }
              onClick={openSetup}
            />
          </ul>
          <DiscoveryContext value={discovery ?? null}>
            <div className="section-forms">
              <SectionForm
                section="capture"
                title="Video source"
                description="Saving restarts screen sync if it is on."
                adapters={captures}
                saved={settings.capture ?? null}
                revision={revision}
                disabled={!online}
                save={(expected, capture) =>
                  dispatch(
                    application.commands.configureCapture(expected, capture),
                  )
                }
              />
              <SectionForm
                section="output"
                title="Lights"
                description="Saving restarts screen sync if it is on."
                adapters={outputs}
                saved={output}
                revision={revision}
                disabled={!online}
                save={(expected, value) =>
                  dispatch(
                    application.commands.configureOutput(expected, value),
                  )
                }
              />
            </div>
          </DiscoveryContext>
        </section>
        <section
          className="settings-section"
          aria-labelledby="appearance-heading"
        >
          <h2 id="appearance-heading">Appearance</h2>
          <ThemeChoice />
        </section>
        {snapshot.surface_failures.map((failure, index) => (
          // Failures have no identity; the list is replaced whole and rows hold no state.
          // eslint-disable-next-line @eslint-react/no-array-index-key
          <p key={index} className="notice warning">
            A Prysm surface stopped: {failure.message}. Lighting continues.
          </p>
        ))}
        <details className="disclosure diagnostics">
          <summary>Technical details</summary>
          <dl className="disclosure-body">
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
              <dt>Capture rate</dt>
              <dd>
                {snapshot.input?.receiving
                  ? `${Math.round(snapshot.input.fps)} fps`
                  : 'no video'}
              </dd>
            </div>
            <div>
              <dt>Settings revision</dt>
              <dd>{revision}</dd>
            </div>
          </dl>
        </details>
      </>,
    );

  // Under the headline: what is missing, or the live picture.
  let hero: ReactNode = null;
  if (!configured)
    hero = (
      <div className="setup-needed">
        <p>Prysm needs a video source and lights before screen sync can run.</p>
        <button type="button" className="primary" onClick={openSetup}>
          Set up Prysm
        </button>
      </div>
    );
  else if (preview)
    hero = (
      <PreviewPanel
        client={preview}
        online={online}
        target={snapshot.target}
        session={session}
      />
    );
  return page(
    'portal-page home',
    'Prysm',
    <>
      {notices}
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
        setTarget={(target) => dispatch(application.commands.setTarget(target))}
      >
        {hero}
      </LightingControls>
      <QuickPicture
        processing={settings.processing}
        revision={revision}
        disabled={!online}
        save={saveProcessing}
      />
    </>,
  );
}
