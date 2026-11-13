import type {
  ApplicationSnapshot,
  DiscoveryClient,
} from '@prysm/application-client';
import type { Processing } from '@prysm/contracts/application';
import { adapterFor, type AdapterSection, type AdapterUis } from './adapters';
import { saveDeveloper, useDeveloper } from './developer';
import { useDeviceName } from './device-name';
import { CheckboxField } from './fields';
import { FieldScopeProvider, useFieldErrors, useFieldScope } from './form';
import { BlackBars } from './processing';
import { nameOf } from './profile-model';
import { Group, RowFact, RowLink, RowSwitch } from './rows';
import { currentTheme, themeChoices } from './theme';

/** Where a row of the Settings page goes. */
export type SettingsPlace =
  | 'video'
  | 'lights'
  | 'profiles'
  | 'picture'
  | 'appearance'
  | 'about'
  | 'setup';

/**
 * The video source or the lights, as one row: named by discovery when it
 * can be, with its few facts, or its live problem in their place.
 */
function SetupRow<S extends AdapterSection>({
  section,
  adapters,
  saved,
  discovery,
  problem,
  open,
}: {
  section: 'capture' | 'output';
  adapters: AdapterUis<S>;
  saved: S;
  discovery: DiscoveryClient | null;
  problem: string | null;
  open: () => void;
}) {
  const adapter = adapterFor(adapters, saved.kind);
  const label = adapter?.label ?? saved.kind;
  const facts = adapter?.facts?.(saved) ?? [];
  const name = useDeviceName(discovery, { section, saved, label, facts });
  // The lights by their LED count, as on Home; a source by size and rate.
  const edges = adapter?.edges?.(saved);
  const details = edges
    ? [`${edges.top + edges.right + edges.bottom + edges.left} LEDs`]
    : facts
        // As on Home: whether TV menus are included is not a fact to list.
        .filter((fact) => fact.label !== 'Menus')
        .map((fact) => fact.value);
  return (
    <RowLink
      label={section === 'capture' ? 'Video source' : 'Lights'}
      value={[name.split('-').join('‑'), ...details].join(' · ')}
      problem={problem}
      onClick={open}
    />
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
  /** The live problem of each side, in plain words; `null` while it works. */
  problemAt: (section: 'capture' | 'output') => string | null;
  /** Save the processing section; light with one while a slider is held. */
  save: (expected: number, processing: Processing) => Promise<number>;
  tryProcessing: (processing: Processing) => Promise<unknown>;
  /** Go to another page. */
  open: (place: SettingsPlace) => void;
}

/**
 * Settings: one column of rows in three groups. Setup (the video source and
 * the lights, each changed on its own page), Picture (profiles, fine-tuning,
 * black bars) and this device. Everything here saves as it changes.
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
  let profileLine = 'None yet';
  if (profiles.length)
    profileLine = `${profiles.length} ${profiles.length === 1 ? 'profile' : 'profiles'} · ${active ? `${nameOf(active)} in use` : 'none in use'}`;
  const theme = themeChoices.find(({ value }) => value === currentTheme());
  return (
    <>
      <Group title="Setup">
        {capture ? (
          <SetupRow
            section="capture"
            adapters={captures}
            saved={capture}
            discovery={discovery}
            problem={problemAt('capture')}
            open={() => open('video')}
          />
        ) : (
          <RowLink
            label="Video source"
            value="Not set up yet"
            onClick={() => open('video')}
          />
        )}
        {output ? (
          <SetupRow
            section="output"
            adapters={outputs}
            saved={output}
            discovery={discovery}
            problem={problemAt('output')}
            open={() => open('lights')}
          />
        ) : (
          <RowLink
            label="Lights"
            value="Not set up yet"
            onClick={() => open('lights')}
          />
        )}
      </Group>
      <Group title="Picture">
        <RowLink
          label="Colour profiles"
          value={profileLine}
          onClick={() => open('profiles')}
        />
        <RowLink
          label="Fine-tuning"
          value="Shadow lift, edge depth, smoothing, detail"
          onClick={() => open('picture')}
        />
        <RowSwitch>
          <BlackBars
            processing={processing}
            revision={revision}
            disabled={!online}
            save={save}
            tryProcessing={tryProcessing}
          />
        </RowSwitch>
      </Group>
      <Group title="This device">
        <RowLink
          label="Appearance"
          value={theme?.label ?? 'System'}
          onClick={() => open('appearance')}
        />
        <RowLink
          label="About this Prysm"
          value="Status, settings version and developer options"
          onClick={() => open('about')}
        />
        <RowLink
          label="Start over with setup"
          value="Choose the video source and lights again, step by step"
          onClick={() => open('setup')}
        />
      </Group>
    </>
  );
}

/** The lighting mode and session, in words. */
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
export function AboutPage({
  snapshot,
}: {
  snapshot: ApplicationSnapshot<unknown, unknown>;
}) {
  const developer = useDeveloper();
  const errors = useFieldErrors();
  const scope = useFieldScope(false, errors, false);
  return (
    <>
      <Group title="Now">
        <RowFact label="Lights" value={targetLabels[snapshot.target]} />
        <RowFact label="Status" value={sessionLabels[snapshot.session.state]} />
        <RowFact
          label="Frames per second"
          value={
            snapshot.input?.receiving
              ? String(Math.round(snapshot.input.fps))
              : 'No picture'
          }
        />
        <RowFact label="Settings version" value={String(snapshot.revision)} />
      </Group>
      <Group title="This browser">
        <RowSwitch>
          <FieldScopeProvider value={scope}>
            <CheckboxField
              path="developer"
              label="Developer options"
              help="Also offers a video file and “No lights” as a setup, for testing."
              checked={developer}
              onChange={saveDeveloper}
            />
          </FieldScopeProvider>
        </RowSwitch>
      </Group>
    </>
  );
}
