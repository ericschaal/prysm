import type { ApplicationSnapshot } from '@prysm/application-client';
import { plainFailure } from '@prysm/application-client';
import { createContext, use, useEffect, useRef, useState } from 'react';
import {
  createHashRouter,
  Outlet,
  RouterProvider,
  ScrollRestoration,
  useLocation,
  useMatches,
  type RouteObject,
} from 'react-router';
import {
  useApplication,
  useApplicationDispatch,
} from '@prysm/application-react';
import type { AdapterSection } from './adapters';
import { useDeveloper } from './developer';
import { DiscoveryContext } from './discovery-context';
import { useLingering } from './lingering';
import { useLiveColors, usePageVisible } from './live-colors';
import { Page } from './page';
import {
  AboutRoute,
  AppearanceRoute,
  CalibrateRoute,
  EditorRoute,
  HomeRoute,
  PictureRoute,
  PreviewRoute,
  ProfileRoute,
  ProfilesRoute,
  SettingsRoute,
} from './pages';
import {
  PortalContext,
  RefsContext,
  useNav,
  type Chrome,
  type Host,
  type Portal,
  type WebPortalProps,
} from './portal-context';
import type { DeletedProfile } from './profiles';
import {
  aboutRoute,
  appearanceRoute,
  calibrateRoute,
  lightsRoute,
  pictureRoute,
  previewRoute,
  profilesRoute,
  settingsRoute,
  videoRoute,
} from './routes';
import { SetupWizard } from './setup';
import { useReplaced } from './replaced';
import { PortalError } from './update';
import type { PathState } from './setup-card';
import './tokens.css';
import './portal.css';
import './flows.css';

/**
 * How long the connection may be missing before the page says so: the first
 * state usually arrives within a few hundred milliseconds of loading (more
 * from a TV), and a restart reconnects in about as long. Saying it at once
 * would flash a warning and swap the header's Settings button in and out.
 */
export const connectionGraceMs = 1000;

type Snapshot = ApplicationSnapshot<AdapterSection, AdapterSection>;

/** What the signal path is doing, from the state alone. */
function pathOf(snapshot: Snapshot): PathState {
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

/**
 * The frame of every page: the state, setup when it takes over, and the page
 * the hash names. Each page is its own: a new one mounts in place of the
 * last (keyed by its address), so nothing a page holds carries over to the
 * next. The router keeps each history entry's scroll place: a new page
 * starts at its top, going back returns to where it was.
 */
function PortalFrame({ host }: { host: Host }) {
  const { application, discovery, colors } = host;
  const connection = useApplication(application);
  const dispatch = useApplicationDispatch<AdapterSection, AdapterSection>();
  // Keep showing the last state (and every draft) while reconnecting.
  const [last, setLast] = useState<Snapshot | null>(null);
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
  const [setupNotice, setSetupNotice] = useState('');
  const developer = useDeveloper();
  const nav = useNav();
  const location = useLocation();
  const matches = useMatches();
  const page = matches[matches.length - 1]?.id;
  const home = page === 'home' || page === 'elsewhere';
  const [previous, setPrevious] = useState<Portal['previous']>({
    capture: null,
    output: null,
  });
  const [deletedProfiles, setDeletedProfiles] = useState<
    Record<string, DeletedProfile>
  >({});
  const noticeRef = useRef<HTMLParagraphElement>(null);
  const homeGridRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (setupNotice) noticeRef.current?.focus();
  }, [setupNotice]);

  // Home's glow takes the LEDs' colours while screen sync runs and is seen.
  const visible = usePageVisible();
  const syncing =
    snapshot?.target === 'processed' && snapshot.session.state === 'running';
  useLiveColors(
    colors,
    home && setup === 'closed' && online && visible && syncing,
    homeGridRef,
  );

  // Disabled at once while offline, but said only once it lasts.
  const outOfTouch = useLingering(!online, connectionGraceMs, 0);
  let connectionLabel = 'Offline';
  if (online) connectionLabel = 'Connected';
  else if (connection.status === 'connecting') connectionLabel = 'Connecting…';
  else if (snapshot) connectionLabel = 'Reconnecting…';
  const chrome: Chrome = {
    connectionLabel,
    outOfTouch,
    settingsLink:
      home && snapshot && setup !== 'open' ? () => nav.go(settingsRoute) : null,
    developer,
    updated: useReplaced(online, host.replaced),
  };

  if (!snapshot)
    return (
      <PortalContext value={{ portal: null, chrome }}>
        <Page className="portal-flow" title="Prysm">
          <section className="connecting">
            <h2>Connecting to Prysm</h2>
            <p>
              {connection.error?.message ?? 'Waiting for the current state…'}
            </p>
          </section>
        </Page>
      </PortalContext>
    );

  const offline = outOfTouch ? (
    <p className="notice warning" role="alert">
      Lost the connection to Prysm. Reconnecting; your changes are kept.
    </p>
  ) : null;
  const path = pathOf(snapshot);
  const { commands } = application;
  const portal: Portal = {
    host,
    snapshot,
    online,
    commands,
    dispatch,
    nav,
    path,
    problemAt: (section) =>
      path.kind === 'failed' && path.at === section && snapshot.failure
        ? plainFailure(snapshot.failure, snapshot.target, 'Device effects')
        : null,
    saveProcessing: (expected, processing) =>
      dispatch(commands.configureProcessing(expected, processing)),
    tryProcessing: (processing) => dispatch(commands.tryProcessing(processing)),
    offline,
    setupNotice,
    openSetup: () => {
      setSetupNotice('');
      setSetup('open');
    },
    previous,
    setPrevious,
    deletedProfiles,
    setDeletedProfiles,
  };
  const { settings, revision } = snapshot;
  return (
    <PortalContext value={{ portal, chrome }}>
      <RefsContext value={{ notice: noticeRef, homeGrid: homeGridRef }}>
        <ScrollRestoration />
        {/* Setup takes over whatever the hash says, except Calibrate. */}
        {setup === 'open' && page !== 'calibrate' ? (
          <Page className="portal-flow" title="Prysm setup" name="Setup">
            {offline}
            <DiscoveryContext value={discovery ?? null}>
              <SetupWizard
                captures={host.captures}
                outputs={host.outputs}
                capture={settings.capture ?? null}
                output={settings.output ?? null}
                revision={revision}
                disabled={!online}
                save={(expected, capture, value) =>
                  dispatch(commands.configureSetup(expected, capture, value))
                }
                start={() => dispatch(commands.setTarget('processed'))}
                close={(notice) => {
                  setSetup('closed');
                  setSetupNotice(notice);
                }}
              />
            </DiscoveryContext>
          </Page>
        ) : (
          <Outlet key={location.pathname} />
        )}
      </RefsContext>
    </PortalContext>
  );
}

/** The host's props, for the frame the router renders. */
const HostContext = createContext<Host | null>(null);

function Frame() {
  const host = use(HostContext);
  if (!host) throw new Error('Outside the portal.');
  return <PortalFrame host={host} />;
}

/** A `#/…` route as a path under the frame. */
const at = (route: string) => route.slice(2);

/** Every page, by its address under the hash; anything else is Home. */
const portalRoutes: RouteObject[] = [
  {
    element: <Frame />,
    errorElement: <PortalError />,
    children: [
      { id: 'home', index: true, element: <HomeRoute /> },
      { id: 'settings', path: at(settingsRoute), element: <SettingsRoute /> },
      {
        id: 'video',
        path: at(videoRoute),
        element: <EditorRoute section="capture" />,
      },
      {
        id: 'lights',
        path: at(lightsRoute),
        element: <EditorRoute section="output" />,
      },
      { id: 'profiles', path: at(profilesRoute), element: <ProfilesRoute /> },
      {
        id: 'profile',
        path: `${at(profilesRoute)}/:id`,
        element: <ProfileRoute />,
      },
      { id: 'picture', path: at(pictureRoute), element: <PictureRoute /> },
      {
        id: 'appearance',
        path: at(appearanceRoute),
        element: <AppearanceRoute />,
      },
      { id: 'about', path: at(aboutRoute), element: <AboutRoute /> },
      { id: 'preview', path: at(previewRoute), element: <PreviewRoute /> },
      {
        id: 'calibrate',
        path: at(calibrateRoute),
        element: <CalibrateRoute />,
      },
      { id: 'elsewhere', path: '*', element: <HomeRoute /> },
    ],
  },
];

/**
 * The browser portal. Home holds the light card (power, mode, brightness,
 * colour profile) and a summary of the setup; Settings (`#/settings`) holds
 * the picture tuning, the capture and lights forms, the live preview
 * (`#/preview`) and the technical details; the Calibrate page is at
 * `#/calibrate`; the guided setup takes over while capture or lights are
 * missing. The host chooses which capture and output kinds exist.
 */
export function WebPortal<C extends AdapterSection, O extends AdapterSection>(
  props: WebPortalProps<C, O>,
) {
  const [router] = useState(() => createHashRouter(portalRoutes));
  return (
    <HostContext value={props as unknown as Host}>
      <RouterProvider router={router} />
    </HostContext>
  );
}
