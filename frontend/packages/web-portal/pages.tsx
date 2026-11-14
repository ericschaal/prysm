import type { Processing } from '@prysm/contracts/application';
import type {
  ApplicationSnapshot,
  DiscoveryClient,
  LightingTarget,
} from '@prysm/application-client';
import type { ReactNode, RefObject } from 'react';
import { useParams } from 'react-router';
import { adapterFor, type AdapterSection, type AdapterUis } from './adapters';
import { CalibrateWizard } from './calibrate';
import { DiscoveryContext } from './discovery-context';
import { LightCard } from './lighting';
import { Page, PageHeading, SetupNotice } from './page';
import { usePortal, usePortalRefs } from './portal-context';
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
import { PreviewPanel } from './preview';
import { FineTuning, QuickPicture } from './processing';
import { nameOf } from './profile-model';
import { ProfileList, ProfilePage } from './profiles';
import { SaveStatus, SaveStatusProvider } from './save-status';
import { SectionEditor } from './section-editor';
import { SetupCard, type PathState } from './setup-card';
import { AboutPage, SettingsPage, type SettingsPlace } from './settings-page';
import { stepCopy } from './step-copy';
import { ThemeChoice } from './theme-choice';

/** The active colour profile's name, or `null` without one. */
function activeProfile(processing: Processing) {
  const active = processing.profiles?.find(
    (profile) => profile.id === processing.active_profile,
  );
  return active ? nameOf(active) : null;
}

/** A page under Settings that saves as it changes: its title and status. */
function LivePage({
  className,
  title,
  name,
  heading,
  to = settingsRoute,
  backLabel = 'Settings',
  note,
  children,
}: {
  className: string;
  title: string;
  name: string;
  heading: string;
  to?: string;
  backLabel?: string;
  /** Instead of the save status, for a page with nothing to save. */
  note?: string;
  children: ReactNode;
}) {
  const { online } = usePortal();
  return (
    <Page className={`portal-page ${className}`} title={title} name={name}>
      <SaveStatusProvider>
        <PageHeading title={heading} to={to} backLabel={backLabel} />
        {/* Choices kept in this browser have nothing to save: said so, in
            the same place, so every page starts alike. */}
        {note !== undefined ? (
          <p className="save-status" data-phase="local">
            {note}
          </p>
        ) : (
          <SaveStatus online={online} />
        )}
        {children}
      </SaveStatusProvider>
    </Page>
  );
}

/** The processing a live page edits, and how it saves and tries. */
function useLiveProps() {
  const { snapshot, online, saveProcessing, tryProcessing } = usePortal();
  return {
    processing: snapshot.settings.processing,
    revision: snapshot.revision,
    disabled: !online,
    save: saveProcessing,
    tryProcessing,
  };
}

export function HomeRoute() {
  const portal = usePortal();
  const { homeGrid } = usePortalRefs();
  const { snapshot, host, online, dispatch, commands, nav } = portal;
  return (
    <Page className="portal-page home" title="Prysm" wide>
      {portal.offline}
      <SetupNotice />
      <HomePage
        snapshot={snapshot}
        captures={host.captures}
        outputs={host.outputs}
        discovery={host.discovery ?? null}
        hasColors={!!host.colors}
        gridRef={homeGrid}
        online={online}
        path={portal.path}
        setTarget={(target) => dispatch(commands.setTarget(target))}
        save={portal.saveProcessing}
        tryProcessing={portal.tryProcessing}
        openSetup={portal.openSetup}
        calibrate={() => nav.go(calibrateRoute)}
        go={nav.go}
      />
    </Page>
  );
}

export function SettingsRoute() {
  const portal = usePortal();
  const { snapshot, host, online, nav } = portal;
  const places: Record<SettingsPlace, () => void> = {
    video: () => nav.go(videoRoute),
    lights: () => nav.go(lightsRoute),
    profiles: () => nav.go(profilesRoute),
    picture: () => nav.go(pictureRoute),
    appearance: () => nav.go(appearanceRoute),
    about: () => nav.go(aboutRoute),
    setup: portal.openSetup,
  };
  return (
    <Page
      className="portal-page settings"
      title="Prysm settings"
      name="Settings"
    >
      <SaveStatusProvider>
        <PageHeading title="Settings" to="" backLabel="Home" />
        <SaveStatus online={online} />
        <SetupNotice />
        {snapshot.surface_failures.length ? (
          <p className="notice warning">
            Part of Prysm’s web service stopped. The lights keep working.
          </p>
        ) : null}
        <SettingsPage
          captures={host.captures}
          outputs={host.outputs}
          snapshot={snapshot}
          online={online}
          discovery={host.discovery ?? null}
          problemAt={portal.problemAt}
          save={portal.saveProcessing}
          tryProcessing={portal.tryProcessing}
          open={(place) => places[place]()}
        />
      </SaveStatusProvider>
    </Page>
  );
}

/** The video source or lights editor, on a page of its own. */
export function EditorRoute({ section }: { section: 'capture' | 'output' }) {
  const portal = usePortal();
  const { snapshot, host, online, dispatch, commands, nav } = portal;
  const { settings } = snapshot;
  const copy = stepCopy[section];
  const shared = {
    revision: snapshot.revision,
    disabled: !online,
    target: snapshot.target,
    path: portal.path,
    failure: snapshot.failure ?? null,
    startSync: () => dispatch(commands.setTarget('processed')),
    leave: () => nav.back(settingsRoute),
    showPreview: host.preview ? () => nav.go(previewRoute) : undefined,
  };
  return (
    <Page
      className="portal-page section-page"
      title={section === 'capture' ? 'Prysm video source' : 'Prysm lights'}
      name={copy.title}
    >
      <PageHeading title={copy.title} to={settingsRoute} backLabel="Settings" />
      {/* In the save line's place: how this page saves. */}
      <p className="save-status" data-phase="local">
        Saved when you choose “{copy.use}”
      </p>
      {portal.offline}
      <DiscoveryContext value={host.discovery ?? null}>
        {section === 'capture' ? (
          <SectionEditor
            section="capture"
            adapters={host.captures}
            saved={settings.capture ?? null}
            previous={portal.previous.capture}
            setPrevious={(capture) =>
              portal.setPrevious({ ...portal.previous, capture })
            }
            save={(expected, capture) =>
              dispatch(commands.configureCapture(expected, capture))
            }
            {...shared}
          />
        ) : (
          <SectionEditor
            section="output"
            adapters={host.outputs}
            saved={settings.output ?? null}
            previous={portal.previous.output}
            setPrevious={(output) =>
              portal.setPrevious({ ...portal.previous, output })
            }
            save={(expected, output) =>
              dispatch(commands.configureOutput(expected, output))
            }
            {...shared}
          />
        )}
      </DiscoveryContext>
    </Page>
  );
}

export function ProfilesRoute() {
  const portal = usePortal();
  return (
    <LivePage
      className="profiles-page"
      title="Prysm colour profiles"
      name="Colour profiles"
      heading="Colour profiles"
    >
      <ProfileList
        {...useLiveProps()}
        deleted={portal.deletedProfiles}
        setDeleted={portal.setDeletedProfiles}
        open={(id) =>
          portal.nav.go(`${profilesRoute}/${encodeURIComponent(id)}`)
        }
        calibrate={() => portal.nav.go(calibrateRoute)}
      />
    </LivePage>
  );
}

export function ProfileRoute() {
  const portal = usePortal();
  const live = useLiveProps();
  const id = useParams().id ?? '';
  const profile = live.processing.profiles?.find((item) => item.id === id);
  const heading = profile ? nameOf(profile) : 'Colour profile';
  return (
    <LivePage
      className="profile-page"
      title="Prysm colour profile"
      name={heading}
      heading={heading}
      to={profilesRoute}
      backLabel="Colour profiles"
    >
      <ProfilePage
        {...live}
        id={id}
        deleted={(gone) =>
          portal.setDeletedProfiles((current) => ({ ...current, [id]: gone }))
        }
        leave={() => portal.nav.back(profilesRoute)}
      />
    </LivePage>
  );
}

export function PictureRoute() {
  return (
    <LivePage
      className="picture-page"
      title="Prysm picture fine-tuning"
      name="Fine-tuning"
      heading="Picture fine-tuning"
    >
      <FineTuning {...useLiveProps()} />
    </LivePage>
  );
}

export function AppearanceRoute() {
  return (
    <LivePage
      className="appearance-page"
      title="Prysm appearance"
      name="Appearance"
      heading="Appearance"
      note="Kept in this browser"
    >
      <ThemeChoice />
    </LivePage>
  );
}

export function AboutRoute() {
  const { snapshot } = usePortal();
  return (
    <LivePage
      className="about-page"
      title="About this Prysm"
      name="About this Prysm"
      heading="About this Prysm"
      note=""
    >
      <AboutPage snapshot={snapshot} />
    </LivePage>
  );
}

export function PreviewRoute() {
  const { snapshot, host, online, offline } = usePortal();
  // Without a preview client there is no Preview page: Home instead.
  if (!host.preview) return <HomeRoute />;
  return (
    <Page
      className="portal-page preview-page"
      title="Prysm live preview"
      name="Live preview"
    >
      <PageHeading
        title="Live preview"
        to={settingsRoute}
        backLabel="Settings"
      />
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
        client={host.preview}
        online={online}
        target={snapshot.target}
        session={snapshot.session}
      />
    </Page>
  );
}

export function CalibrateRoute() {
  const { snapshot, online, dispatch, commands, nav, offline, saveProcessing } =
    usePortal();
  return (
    <Page
      className="portal-flow portal-flow-narrow"
      title="Prysm calibration"
      name="Calibrate"
    >
      {offline}
      <CalibrateWizard
        processing={snapshot.settings.processing}
        revision={snapshot.revision}
        target={snapshot.target}
        calibration={snapshot.calibration}
        disabled={!online}
        save={saveProcessing}
        setCalibration={(calibration) =>
          dispatch(commands.setCalibration(calibration))
        }
        // Back where it was opened (Home, or the profiles), else Home.
        leave={() => nav.back(nav.from ?? '')}
      />
    </Page>
  );
}

/**
 * Home: the light card (power, mode, brightness, colour profile) and, once
 * set up, the path the light takes. `gridRef` takes the live LED colours.
 */
function HomePage<C extends AdapterSection, O extends AdapterSection>({
  snapshot,
  captures,
  outputs,
  discovery,
  hasColors,
  gridRef,
  online,
  path,
  setTarget,
  save,
  tryProcessing,
  openSetup,
  calibrate,
  go,
}: {
  snapshot: ApplicationSnapshot<C, O>;
  captures: AdapterUis<C>;
  outputs: AdapterUis<O>;
  discovery: DiscoveryClient | null;
  /** Whether this host streams the LEDs' colours at all. */
  hasColors: boolean;
  gridRef: RefObject<HTMLDivElement | null>;
  online: boolean;
  path: PathState;
  setTarget: (target: LightingTarget) => Promise<unknown>;
  save: (expected: number, processing: Processing) => Promise<number>;
  tryProcessing: (processing: Processing) => Promise<unknown>;
  openSetup: () => void;
  calibrate: () => void;
  go: (route: string) => void;
}) {
  const { settings, revision, session } = snapshot;
  const capture = settings.capture ?? null;
  const output = settings.output ?? null;
  const captureUi = capture ? adapterFor(captures, capture.kind) : undefined;
  const outputUi = output ? adapterFor(outputs, output.kind) : undefined;
  return (
    <div
      className="home-grid"
      ref={gridRef}
      data-colors={hasColors ? 'stream' : 'none'}
    >
      <LightCard
        target={snapshot.target}
        session={session}
        failure={snapshot.failure}
        outputIssue={snapshot.output_issue}
        outputBrightness={snapshot.output_brightness}
        input={snapshot.input}
        hasCapture={!!capture}
        hasOutput={!!output}
        output={output ? (outputUi?.lighting?.(output) ?? {}) : null}
        disabled={!online}
        setTarget={setTarget}
        openSetup={openSetup}
      >
        <QuickPicture
          processing={settings.processing}
          revision={revision}
          disabled={!online}
          save={save}
          calibrate={calibrate}
          tryProcessing={tryProcessing}
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
          discovery={discovery}
          path={path}
          profile={activeProfile(settings.processing)}
          openSettings={() => {
            if (path.kind === 'failed' && path.at === 'capture') go(videoRoute);
            else if (path.kind === 'failed' && path.at === 'output')
              go(lightsRoute);
            else go(settingsRoute);
          }}
        />
      ) : null}
    </div>
  );
}
