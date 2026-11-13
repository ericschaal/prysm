import { useId, type ReactNode } from 'react';
import type {
  ApplicationSnapshot,
  DiscoveryClient,
} from '@prysm/application-client';
import type { Processing } from '@prysm/contracts/application';
import { adapterFor, type AdapterSection, type AdapterUis } from './adapters';
import { useDeveloper, saveDeveloper } from './developer';
import { useDeviceName } from './device-name';
import { CheckboxField } from './fields';
import { FieldScopeProvider, useFieldErrors, useFieldScope } from './form';
import { LightsIcon, PrysmIcon, VideoIcon } from './path-icons';
import { BlackBars } from './processing';
import { nameOf } from './profile-model';
import { ThemeChoice } from './theme-choice';

/** Where a link on the Settings page goes. */
export type SettingsPlace =
  'video' | 'lights' | 'profiles' | 'picture' | 'preview' | 'setup';

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

/** A row that opens a page: what it is, one line about it, a chevron. */
function CardLink({
  label,
  description,
  onClick,
}: {
  label: string;
  description: string;
  onClick: () => void;
}) {
  const id = useId();
  return (
    <button
      type="button"
      className="card-link"
      aria-describedby={id}
      onClick={onClick}
    >
      <span className="card-link-text">
        <span className="card-link-label">{label}</span>
        <span id={id} className="card-link-description">
          {description}
        </span>
      </span>
      <ChevronIcon />
    </button>
  );
}

/**
 * One stop of the signal path, as a card: what it is, its name and one fact,
 * a line kept for its live problem, then what can be done with it.
 */
function PathCard({
  title,
  icon,
  name,
  fact,
  problem,
  children,
}: {
  title: string;
  icon: ReactNode;
  name: string;
  fact?: string;
  /** The live problem of this stop; `null` while it works. */
  problem: string | null;
  children: ReactNode;
}) {
  const headingId = useId();
  return (
    <section
      className="path-card"
      aria-labelledby={headingId}
      data-failed={problem !== null}
    >
      <h2 id={headingId} className="overline">
        {title}
      </h2>
      <div className="path-card-head">
        <span className="path-icon">{icon}</span>
        <span className="path-card-text">
          <strong className="path-card-name">
            {name.split('-').join('‑')}
          </strong>
          <span className="path-card-fact">{fact ?? ' '}</span>
        </span>
      </div>
      <p className="path-card-problem" role="status">
        {problem}
      </p>
      <div className="path-card-body">{children}</div>
    </section>
  );
}

/** The video source or the lights: named by discovery when it can be. */
function PartCard<S extends AdapterSection>({
  section,
  adapters,
  saved,
  discovery,
  problem,
  open,
  children,
}: {
  section: 'capture' | 'output';
  adapters: AdapterUis<S>;
  saved: S;
  discovery: DiscoveryClient | null;
  problem: string | null;
  open: () => void;
  children?: ReactNode;
}) {
  const adapter = adapterFor(adapters, saved.kind);
  const label = adapter?.label ?? saved.kind;
  const facts = adapter?.facts?.(saved) ?? [];
  const name = useDeviceName(discovery, { section, saved, label, facts });
  // As on Home: the lights' LED count said as a count, then the rest.
  const edges = adapter?.edges?.(saved);
  const total = edges
    ? `${edges.top + edges.right + edges.bottom + edges.left} LEDs`
    : undefined;
  const fact = [
    total,
    ...facts
      .filter((item) => !(total && item.label === 'LEDs'))
      .map((item) => item.value),
  ]
    .filter(Boolean)
    .join(' · ');
  return (
    <PathCard
      title={section === 'capture' ? 'Video source' : 'Lights'}
      icon={section === 'capture' ? <VideoIcon /> : <LightsIcon />}
      name={name}
      fact={fact || undefined}
      problem={problem}
    >
      <div className="path-card-actions">
        <button type="button" onClick={open}>
          Change
        </button>
        {children}
      </div>
    </PathCard>
  );
}

/** A part not set up yet: said so, with the way to set it up. */
function MissingCard({
  section,
  open,
}: {
  section: 'capture' | 'output';
  open: () => void;
}) {
  return (
    <PathCard
      title={section === 'capture' ? 'Video source' : 'Lights'}
      icon={section === 'capture' ? <VideoIcon /> : <LightsIcon />}
      name="Not set up yet"
      problem={null}
    >
      <div className="path-card-actions">
        <button type="button" className="primary" onClick={open}>
          Set up
        </button>
      </div>
    </PathCard>
  );
}

/** The lighting mode and session, in words, for “About this Prysm”. */
const targetLabels = {
  off: 'Off',
  processed: 'Screen sync',
  device_native: 'The lights’ own effects',
} as const satisfies Record<
  ApplicationSnapshot<unknown, unknown>['target'],
  string
>;
const sessionLabels = {
  stopped: 'Stopped',
  running: 'Running',
  native: 'Showing the lights’ own effects',
  faulted: 'Stopped by a fault; restart Prysm',
} as const satisfies Record<
  ApplicationSnapshot<unknown, unknown>['session']['state'],
  string
>;

/** What this Prysm is doing, and the developer options of this browser. */
function About({
  snapshot,
}: {
  snapshot: ApplicationSnapshot<unknown, unknown>;
}) {
  const developer = useDeveloper();
  const errors = useFieldErrors();
  const scope = useFieldScope(false, errors, false);
  return (
    <details className="disclosure about">
      <summary>About this Prysm</summary>
      <div className="disclosure-body">
        <dl className="about-facts">
          <div>
            <dt>Lights</dt>
            <dd>{targetLabels[snapshot.target]}</dd>
          </div>
          <div>
            <dt>Status</dt>
            <dd>{sessionLabels[snapshot.session.state]}</dd>
          </div>
          <div>
            <dt>Frames per second</dt>
            <dd>
              {snapshot.input?.receiving
                ? Math.round(snapshot.input.fps)
                : 'No picture'}
            </dd>
          </div>
          <div>
            <dt>Settings version</dt>
            <dd>{snapshot.revision}</dd>
          </div>
        </dl>
        <FieldScopeProvider value={scope}>
          <CheckboxField
            path="developer"
            label="Developer options"
            help="Also offers a video file and “No lights” as a setup, for testing. Kept in this browser."
            checked={developer}
            onChange={saveDeveloper}
          />
        </FieldScopeProvider>
      </div>
    </details>
  );
}

export interface SettingsPageProps<
  C extends AdapterSection,
  O extends AdapterSection,
> {
  captures: AdapterUis<C>;
  outputs: AdapterUis<O>;
  snapshot: ApplicationSnapshot<C, O>;
  online: boolean;
  discovery: DiscoveryClient | null;
  /** Whether this host has a live preview. */
  preview: boolean;
  /** The live problem of each side, in plain words; `null` while it works. */
  problemAt: (section: 'capture' | 'output' | null) => string | null;
  /** Save the processing section; light with one while a slider is held. */
  save: (expected: number, processing: Processing) => Promise<number>;
  tryProcessing: (processing: Processing) => Promise<unknown>;
  /** Go to another page. */
  open: (place: SettingsPlace) => void;
}

/**
 * Settings, in the order the light takes: the video source, Prysm, the
 * lights; then More. Everything here is saved as it changes; the video
 * source and lights are changed on pages of their own.
 */
export function SettingsPage<
  C extends AdapterSection,
  O extends AdapterSection,
>({
  captures,
  outputs,
  snapshot,
  online,
  discovery,
  preview,
  problemAt,
  save,
  tryProcessing,
  open,
}: SettingsPageProps<C, O>) {
  const { settings, revision } = snapshot;
  const capture = settings.capture ?? null;
  const output = settings.output ?? null;
  const processing = settings.processing;
  const profiles = processing.profiles ?? [];
  const active = profiles.find(({ id }) => id === processing.active_profile);
  let profileLine = 'None yet. Make one with test patterns on the TV.';
  if (profiles.length)
    profileLine = `${profiles.length} ${profiles.length === 1 ? 'profile' : 'profiles'} · ${active ? `${nameOf(active)} in use` : 'none in use'}`;
  return (
    <>
      <div className="settings-path">
        {capture ? (
          <PartCard
            section="capture"
            adapters={captures}
            saved={capture}
            discovery={discovery}
            problem={problemAt('capture')}
            open={() => open('video')}
          >
            {preview ? (
              <button
                type="button"
                className="quiet"
                onClick={() => open('preview')}
              >
                See the picture
              </button>
            ) : null}
          </PartCard>
        ) : (
          <MissingCard section="capture" open={() => open('setup')} />
        )}
        <PathCard
          title="Prysm"
          icon={<PrysmIcon />}
          name="Prysm"
          fact={active ? `${nameOf(active)} profile` : 'No colour correction'}
          problem={problemAt(null)}
        >
          <BlackBars
            processing={processing}
            revision={revision}
            disabled={!online}
            save={save}
            tryProcessing={tryProcessing}
          />
          <div className="card-links">
            <CardLink
              label="Colour profiles"
              description={profileLine}
              onClick={() => open('profiles')}
            />
            <CardLink
              label="Picture fine-tuning"
              description="Shadows, edges, smoothing and detail."
              onClick={() => open('picture')}
            />
          </div>
        </PathCard>
        {output ? (
          <PartCard
            section="output"
            adapters={outputs}
            saved={output}
            discovery={discovery}
            problem={problemAt('output')}
            open={() => open('lights')}
          >
            {preview ? (
              <button
                type="button"
                className="quiet"
                onClick={() => open('preview')}
              >
                Check placement
              </button>
            ) : null}
          </PartCard>
        ) : (
          <MissingCard section="output" open={() => open('setup')} />
        )}
      </div>
      <section className="settings-more" aria-labelledby="more-heading">
        <h2 id="more-heading" className="overline">
          More
        </h2>
        <div className="more-grid">
          <div className="more-card">
            <h3>Appearance</h3>
            <ThemeChoice />
          </div>
          <div className="more-card">
            <About snapshot={snapshot} />
            <CardLink
              label="Start over with setup"
              description="Choose the video source and lights again, step by step."
              onClick={() => open('setup')}
            />
          </div>
        </div>
      </section>
    </>
  );
}
