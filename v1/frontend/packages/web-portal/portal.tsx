import type { ApplicationRuntime } from '@prysm/application-state';
import type { Processing } from '@prysm/contracts/application';
import {
  plainFailure,
  type ApplicationSnapshot,
  type ColorsClient,
  type DiscoveryClient,
  type PreviewClient,
} from '@prysm/application-client';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import {
  useApplication,
  useApplicationDispatch,
} from '@prysm/application-react';
import { adapterFor, type AdapterSection, type AdapterUis } from './adapters';
import { calibrateRoute, CalibrateWizard } from './calibrate';
import { DeveloperContext, useDeveloper } from './developer';
import { DiscoveryContext } from './discovery-context';
import { LightCard } from './lighting';
import { useLiveColors, usePageVisible } from './live-colors';
import { PreviewPanel } from './preview';
import { FineTuning, QuickPicture } from './processing';
import { ProfileList, ProfilePage, type DeletedProfile } from './profiles';
import { SaveStatus, SaveStatusProvider } from './save-status';
import { SectionEditor } from './section-editor';
import { stepCopy } from './step-copy';
import { SetupWizard } from './setup';
import { nameOf } from './profile-model';
import { SetupCard, type PathState } from './setup-card';
import { AboutPage, SettingsPage, type SettingsPlace } from './settings-page';
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
   * The LEDs' colours, for the glow Home casts while screen sync runs;
   * without one the glow is the designed one.
   */
  colors?: ColorsClient;
  /**
   * Finds devices to choose from in the capture and lights editors; without
   * one they offer only typed fields.
   */
  discovery?: DiscoveryClient;
}

/** The hash route of the Settings page. */
export const settingsRoute = '#/settings';
/** The hash route of the live preview, opened from Settings. */
export const previewRoute = '#/preview';
/** The hash routes of the video source and lights editors, under Settings. */
export const videoRoute = '#/settings/video';
export const lightsRoute = '#/settings/lights';
/** The hash routes of the colour profiles and picture fine-tuning pages. */
export const profilesRoute = '#/settings/profiles';
export const pictureRoute = '#/settings/picture';
/** The hash routes of the appearance and about pages. */
export const appearanceRoute = '#/settings/appearance';
export const aboutRoute = '#/settings/about';

type Route =
  | 'home'
  | 'settings'
  | 'calibrate'
  | 'preview'
  | 'video'
  | 'lights'
  | 'profiles'
  | 'picture'
  | 'appearance'
  | 'about'
  | 'profile';

/**
 * Which page the hash names (Home, `#/settings`, `#/calibrate`,
 * `#/preview`) and a way to go to another. The hash keeps the portal's own
 * address and history.
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
  else if (hash === previewRoute) route = 'preview';
  else if (hash === videoRoute) route = 'video';
  else if (hash === lightsRoute) route = 'lights';
  else if (hash === profilesRoute) route = 'profiles';
  else if (hash === pictureRoute) route = 'picture';
  else if (hash === appearanceRoute) route = 'appearance';
  else if (hash === aboutRoute) route = 'about';
  else if (hash.startsWith(`${profilesRoute}/`)) route = 'profile';
  return [route, go, hash] as const;
}

function SettingsIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
      <circle cx="15" cy="7" r="2" />
      <circle cx="9" cy="17" r="2" />
    </svg>
  );
}

/** What the signal path is doing, from the state alone. */
function pathOf(snapshot: ApplicationSnapshot<unknown, unknown>): PathState {
  const { target, session, failure, input } = snapshot;
  if (session.state === 'faulted') return { kind: 'failed', at: null };
  if (target !== 'off' && failure) {
    // Which side failed: typed for adapter failures, else the setting named.
    const field = failure.field?.split('.')[0];
    const named = field === 'capture' || field === 'output' ? field : null;
    return { kind: 'failed', at: failure.section ?? named };
  }
  if (target === 'processed' && session.state === 'running')
    return input?.receiving
      ? {
          kind: 'flowing',
          fps: input.fps,
          latency: input.latency && {
            captureMs: input.latency.capture_ms,
            outputMs: input.latency.output_ms,
          },
          behind: input.behind,
        }
      : { kind: 'waiting' };
  if (target === 'device_native' && session.state === 'native')
    return { kind: 'effects' };
  return { kind: 'off' };
}

/** The active colour profile's name, or `null` without one. */
function activeProfile(processing: Processing) {
  const active = processing.profiles?.find(
    (profile) => profile.id === processing.active_profile,
  );
  return active ? nameOf(active) : null;
}

/**
 * The browser portal. Home holds the light card (power, mode, brightness,
 * colour profile) and a summary of the setup; Settings (`#/settings`) holds
 * the picture tuning, the capture and lights forms, the live preview
 * (`#/preview`) and the technical details; the Calibrate page is at
 * `#/calibrate`; the guided setup takes over while capture or lights are
 * missing. The host chooses which capture and output kinds exist.
 */
export function WebPortal<C extends AdapterSection, O extends AdapterSection>({
  application,
  captures,
  outputs,
  preview,
  colors,
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
  const [hashRoute, go, hash] = useRoute();
  const developer = useDeveloper();
  // Without a preview client there is no Preview page: Home instead.
  const route = hashRoute === 'preview' && !preview ? 'home' : hashRoute;
  /**
   * What each editor saved over last, offered back until it is replaced,
   * also after leaving the editor.
   */
  const [previous, setPrevious] = useState<{
    capture: C | null;
    output: O | null;
  }>({ capture: null, output: null });
  /** Profiles deleted on this visit, shown in their places with Undo. */
  const [deletedProfiles, setDeletedProfiles] = useState<
    Record<string, DeletedProfile>
  >({});
  /** Where Calibrate returns to: Settings when opened from there. */
  const [calibrateFrom, setCalibrateFrom] = useState('');
  const pageHeadingRef = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (setupNotice) noticeRef.current?.focus();
  }, [setupNotice]);
  // Arriving in Settings or Preview: its heading, for screen readers and keyboards.
  useEffect(() => {
    if (route !== 'home' && route !== 'calibrate')
      pageHeadingRef.current?.focus({ preventScroll: true });
  }, [route]);

  // Home's glow takes the LEDs' colours while screen sync runs and is seen.
  const homeGridRef = useRef<HTMLDivElement>(null);
  const visible = usePageVisible();
  const syncing =
    snapshot?.target === 'processed' && snapshot.session.state === 'running';
  useLiveColors(
    colors,
    route === 'home' && setup === 'closed' && online && visible && syncing,
    homeGridRef,
  );

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
  const page = (
    className: string,
    title: string,
    content: ReactNode,
    wide = false,
  ) => (
    <div className={wide ? 'portal wide' : 'portal'}>
      {header}
      <main id="content" className={className}>
        <h1 className="visually-hidden">{title}</h1>
        <DeveloperContext value={developer}>{content}</DeveloperContext>
      </main>
    </div>
  );
  /** A page under Settings: its title and the way back. */
  const pageHeading = (title: string, back: string, backLabel: string) => (
    <div className="page-heading">
      <button type="button" className="back quiet" onClick={() => go(back)}>
        <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
          <path d="M15 5.5 8.5 12l6.5 6.5" />
        </svg>
        {backLabel}
      </button>
      <h2 ref={pageHeadingRef} tabIndex={-1}>
        {title}
      </h2>
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
  const outputLighting = output
    ? (adapterFor(outputs, output.kind)?.lighting?.(output) ?? {})
    : null;
  const offline = !online ? (
    <p className="notice warning" role="alert">
      Lost the connection to Prysm. Reconnecting; your changes are kept.
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

  const path = pathOf(snapshot);
  /** The live problem of one side, in plain words; `null` while it works. */
  const problemAt = (section: 'capture' | 'output') =>
    path.kind === 'failed' && path.at === section && snapshot.failure
      ? plainFailure(snapshot.failure, snapshot.target, 'Device effects')
      : null;
  const startSync = () => dispatch(application.commands.setTarget('processed'));
  const editorProps = {
    revision,
    disabled: !online,
    target: snapshot.target,
    path,
    failure: snapshot.failure ?? null,
    startSync,
    leave: () => go(settingsRoute),
    showPreview: preview ? () => go(previewRoute) : undefined,
  };
  if (route === 'video' || route === 'lights')
    return page(
      'portal-page section-page',
      route === 'video' ? 'Prysm video source' : 'Prysm lights',
      <>
        {pageHeading(
          stepCopy[route === 'video' ? 'capture' : 'output'].title,
          settingsRoute,
          'Settings',
        )}
        {/* In the save line's place: how this page saves. */}
        <p className="save-status" data-phase="local">
          Saved when you choose “
          {stepCopy[route === 'video' ? 'capture' : 'output'].use}”
        </p>
        {offline}
        <DiscoveryContext value={discovery ?? null}>
          {route === 'video' ? (
            <SectionEditor
              section="capture"
              adapters={captures}
              saved={settings.capture ?? null}
              previous={previous.capture}
              setPrevious={(capture) => setPrevious({ ...previous, capture })}
              save={(expected, capture) =>
                dispatch(
                  application.commands.configureCapture(expected, capture),
                )
              }
              {...editorProps}
            />
          ) : (
            <SectionEditor
              section="output"
              adapters={outputs}
              saved={output}
              previous={previous.output}
              setPrevious={(value) =>
                setPrevious({ ...previous, output: value })
              }
              save={(expected, value) =>
                dispatch(application.commands.configureOutput(expected, value))
              }
              {...editorProps}
            />
          )}
        </DiscoveryContext>
      </>,
    );

  const tryProcessing = (processing: Processing) =>
    dispatch(application.commands.tryProcessing(processing));
  /** A page under Settings that saves as it changes: its title and status. */
  const livePage = (
    className: string,
    title: string,
    heading: string,
    content: ReactNode,
    back = settingsRoute,
    backLabel = 'Settings',
    /** Instead of the save status, for a page with nothing to save. */
    note?: string,
  ) =>
    page(
      `portal-page ${className}`,
      title,
      <SaveStatusProvider>
        {pageHeading(heading, back, backLabel)}
        {/* Choices kept in this browser have nothing to save: said so, in
            the same place, so every page starts alike. */}
        {note !== undefined ? (
          <p className="save-status" data-phase="local">
            {note}
          </p>
        ) : (
          <SaveStatus online={online} />
        )}
        {content}
      </SaveStatusProvider>,
    );
  const places: Record<SettingsPlace, () => void> = {
    video: () => go(videoRoute),
    lights: () => go(lightsRoute),
    profiles: () => go(profilesRoute),
    picture: () => go(pictureRoute),
    appearance: () => go(appearanceRoute),
    about: () => go(aboutRoute),
    setup: openSetup,
  };

  const liveProps = {
    processing: settings.processing,
    revision,
    disabled: !online,
    save: saveProcessing,
    tryProcessing,
  };
  if (route === 'profiles')
    return livePage(
      'profiles-page',
      'Prysm colour profiles',
      'Colour profiles',
      <ProfileList
        {...liveProps}
        deleted={deletedProfiles}
        setDeleted={setDeletedProfiles}
        open={(id) => go(`${profilesRoute}/${encodeURIComponent(id)}`)}
        calibrate={() => {
          setCalibrateFrom(profilesRoute);
          go(calibrateRoute);
        }}
      />,
    );
  if (route === 'profile') {
    const id = decodeURIComponent(hash.slice(profilesRoute.length + 1));
    const profile = settings.processing.profiles?.find(
      (item) => item.id === id,
    );
    return livePage(
      'profile-page',
      'Prysm colour profile',
      profile ? nameOf(profile) : 'Colour profile',
      <ProfilePage
        {...liveProps}
        id={id}
        deleted={(gone) =>
          setDeletedProfiles((current) => ({ ...current, [id]: gone }))
        }
        leave={() => go(profilesRoute)}
      />,
      profilesRoute,
      'Colour profiles',
    );
  }
  if (route === 'appearance')
    return livePage(
      'appearance-page',
      'Prysm appearance',
      'Appearance',
      <ThemeChoice />,
      settingsRoute,
      'Settings',
      'Kept in this browser',
    );
  if (route === 'about')
    return livePage(
      'about-page',
      'About this Prysm',
      'About this Prysm',
      <AboutPage snapshot={snapshot} />,
      settingsRoute,
      'Settings',
      '',
    );
  if (route === 'picture')
    return livePage(
      'picture-page',
      'Prysm picture fine-tuning',
      'Picture fine-tuning',
      <FineTuning
        processing={settings.processing}
        revision={revision}
        disabled={!online}
        save={saveProcessing}
        tryProcessing={tryProcessing}
      />,
    );

  if (route === 'settings')
    return page(
      'portal-page settings',
      'Prysm settings',
      <SaveStatusProvider>
        {pageHeading('Settings', '', 'Home')}
        <SaveStatus online={online} />
        {setupNotice ? (
          <p className="notice" role="status" tabIndex={-1} ref={noticeRef}>
            {setupNotice}
          </p>
        ) : null}
        {snapshot.surface_failures.length ? (
          <p className="notice warning">
            Part of Prysm’s web service stopped. The lights keep working.
          </p>
        ) : null}
        <SettingsPage
          captures={captures}
          outputs={outputs}
          snapshot={snapshot}
          online={online}
          discovery={discovery ?? null}
          problemAt={problemAt}
          save={saveProcessing}
          tryProcessing={tryProcessing}
          open={(place) => places[place]()}
        />
      </SaveStatusProvider>,
    );

  if (route === 'preview' && preview)
    return page(
      'portal-page preview-page',
      'Prysm live preview',
      <>
        {pageHeading('Live preview', settingsRoute, 'Settings')}
        {/* In the save line's place, so every page starts alike. */}
        <p className="save-status" data-phase="local">
          Live while this page is open
        </p>
        <p className="page-intro">
          What Prysm captures, with the colours it sends to each edge around it.
          Check black bars and LED placement here.
        </p>
        {offline}
        <PreviewPanel
          client={preview}
          online={online}
          target={snapshot.target}
          session={session}
        />
      </>,
    );

  const capture = settings.capture ?? null;
  const captureUi = capture ? adapterFor(captures, capture.kind) : undefined;
  const outputUi = output ? adapterFor(outputs, output.kind) : undefined;
  const calibrate = () => {
    setCalibrateFrom('');
    go(calibrateRoute);
  };
  return page(
    'portal-page home',
    'Prysm',
    <>
      <p className="display">
        Light, in sync<span aria-hidden="true">.</span>
      </p>
      {notices}
      {/* Whether this host streams the LEDs' colours at all. */}
      <div
        className="home-grid"
        ref={homeGridRef}
        data-colors={colors ? 'stream' : 'none'}
      >
        <LightCard
          target={snapshot.target}
          session={session}
          failure={snapshot.failure}
          outputIssue={snapshot.output_issue}
          input={snapshot.input}
          hasCapture={!!capture}
          hasOutput={!!output}
          output={outputLighting}
          disabled={!online}
          setTarget={(target) =>
            dispatch(application.commands.setTarget(target))
          }
          openSetup={openSetup}
        >
          <QuickPicture
            processing={settings.processing}
            revision={revision}
            disabled={!online}
            save={saveProcessing}
            calibrate={calibrate}
            tryProcessing={(processing) =>
              dispatch(application.commands.tryProcessing(processing))
            }
          />
        </LightCard>
        {/* Before setup the light card's prompt says it all. */}
        {capture && output ? (
          <SetupCard
            capture={{
              section: 'capture',
              saved: capture,
              label: captureUi?.label ?? capture.kind,
              facts: captureUi?.facts?.(capture) ?? [],
            }}
            output={{
              section: 'output',
              saved: output,
              label: outputUi?.label ?? output.kind,
              facts: outputUi?.facts?.(output) ?? [],
            }}
            edges={outputUi?.edges?.(output)}
            transports={{
              capture: captureUi?.transport?.(capture),
              output: outputUi?.transport?.(output),
            }}
            discovery={discovery ?? null}
            path={path}
            profile={activeProfile(settings.processing)}
            openSettings={() => {
              if (path.kind === 'failed' && path.at === 'capture')
                go(videoRoute);
              else if (path.kind === 'failed' && path.at === 'output')
                go(lightsRoute);
              else go(settingsRoute);
            }}
          />
        ) : null}
      </div>
    </>,
    true,
  );
}
