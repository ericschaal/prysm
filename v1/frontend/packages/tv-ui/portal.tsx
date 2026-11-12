/**
 * The Prysm that drives the lights, over its HTTP API: the full package's own
 * portal on this TV, or the slim package's remote Prysm (`preferences.instance`).
 */
import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
  type FormEvent,
} from 'react';
import {
  ApplicationError,
  type ApplicationClient,
  type ApplicationSnapshot,
  type Failure,
  type LightingTarget,
} from '@prysm/application-client';
import {
  createApplicationRuntime,
  type ApplicationRuntime,
  type Connection,
} from '@prysm/application-state';
import { equal } from './model';

type Snapshot = ApplicationSnapshot<unknown, unknown>;
type Runtime = ApplicationRuntime<unknown, unknown>;

export interface PortalOptions {
  /** An application client for the portal at `baseUrl` (an origin). */
  client: (baseUrl: string) => ApplicationClient<unknown, unknown>;
  /**
   * Whether the portal answers at all, ignoring CORS (a `no-cors` request):
   * tells "not allowed from the TV" from "not reachable".
   */
  probe?: (baseUrl: string) => Promise<boolean>;
  /** The browser has what the HTTP client needs; default: detected. */
  supported?: boolean;
}

/** What the HTTP client and the state runtime use beyond Chromium 53. */
export function portalSupported(): boolean {
  const scope = window as unknown as Record<string, unknown>;
  return (
    typeof scope.AbortController === 'function' &&
    typeof scope.TextDecoder === 'function' &&
    typeof scope.ReadableStream === 'function' &&
    typeof Promise.prototype.finally === 'function'
  );
}

export type Reach = 'unknown' | 'refused' | 'unreachable';

export function failureOf(error: unknown): Failure {
  return error instanceof ApplicationError
    ? error.failure
    : {
        kind: 'internal',
        message: error instanceof Error ? error.message : String(error),
      };
}

/** The state stream of the portal at `baseUrl` while `active`. */
export function usePortal(
  baseUrl: string | null,
  options: PortalOptions,
  active: boolean,
) {
  const supported = options.supported ?? portalSupported();
  const { client, probe } = options;
  const runtime = useMemo<Runtime | null>(
    () => (baseUrl && supported ? createApplicationRuntime(client(baseUrl)) : null),
    [baseUrl, supported, client],
  );
  const subscribe = useCallback(
    (changed: () => void) => (runtime ? runtime.store.subscribe(changed) : () => {}),
    [runtime],
  );
  const connection = useSyncExternalStore(subscribe, () =>
    runtime ? runtime.selectors.selectConnection(runtime.store.getState()) : null,
  ) as Connection<unknown, unknown> | null;
  useEffect(() => (runtime && active ? runtime.connect() : undefined), [runtime, active]);

  const [reach, setReach] = useState<{ error: string; reach: Reach } | null>(null);
  // Kept while the runtime retries, until a state arrives.
  const error = connection?.status === 'connected' ? null : (connection?.error ?? null);
  useEffect(() => {
    if (!error || !baseUrl) return;
    let current = true;
    if (/refuses requests from this page|origin denied/i.test(error)) {
      setReach({ error, reach: 'refused' });
      return;
    }
    if (!probe) return;
    probe(baseUrl).then(
      (reachable) => {
        if (current) setReach({ error, reach: reachable ? 'refused' : 'unreachable' });
      },
      () => {
        if (current) setReach({ error, reach: 'unreachable' });
      },
    );
    return () => {
      current = false;
    };
  }, [error, baseUrl, probe]);

  const snapshot: Snapshot | null =
    connection?.status === 'connected' ? connection.snapshot : null;
  return {
    baseUrl,
    supported,
    runtime,
    connection,
    snapshot,
    reach: error && reach?.error === error ? reach.reach : ('unknown' as Reach),
    setTarget: (target: LightingTarget) =>
      runtime!.store.dispatch(runtime!.commands.setTarget(target)),
    configureCapture: (expected: number, capture: unknown) =>
      runtime!.store.dispatch(runtime!.commands.configureCapture(expected, capture)),
    configureOutput: (expected: number, output: unknown) =>
      runtime!.store.dispatch(runtime!.commands.configureOutput(expected, output)),
    /** The latest observed state, read at the moment of a save. */
    latest: () =>
      runtime ? runtime.selectors.selectSnapshot(runtime.store.getState()) : null,
  };
}
export type PortalView = ReturnType<typeof usePortal>;

const outputKind = (snapshot: Snapshot) =>
  (snapshot.settings.output as { kind?: string } | undefined)?.kind;
const effectsOf = (snapshot: Snapshot) =>
  outputKind(snapshot) === 'wled' ? 'WLED effects' : null;

export function describeLighting(snapshot: Snapshot): string {
  const effects = effectsOf(snapshot) ?? 'Light effects';
  const { session, target, failure } = snapshot;
  switch (session.state) {
    case 'faulted':
      return 'Prysm stopped responding. Restart Prysm to recover.';
    case 'running':
      return 'Screen sync is on.';
    case 'native':
      return `${effects} are on.`;
    case 'stopped':
      if (target === 'off') return 'Lights are off.';
      if (failure) return target === 'processed' ? 'Screen sync stopped.' : `${effects} could not start.`;
      return target === 'processed' ? 'Starting screen sync…' : `Starting ${effects}…`;
  }
}

const captureNames: Record<string, string> = {
  remote: 'a TV (Remote capture)',
  device: 'a video device',
  file: 'a video file',
  webos: 'this TV',
};

/** The lighting card: what the lights do and the three modes. */
export function LightingCard({
  flavor,
  portal,
  instance,
  portalAddress,
}: {
  flavor: 'full' | 'slim';
  portal: PortalView;
  /** Slim: the Prysm address being used (draft or saved). */
  instance: string | null;
  /** Full: the advertised portal address, for people on other devices. */
  portalAddress: string | null;
}) {
  const [pending, setPending] = useState<LightingTarget | null>(null);
  const [error, setError] = useState('');
  const { snapshot, connection } = portal;
  const where = flavor === 'full' ? 'the Prysm on this TV' : `Prysm at ${instance ?? ''}`;

  async function choose(target: LightingTarget) {
    setPending(target);
    setError('');
    try {
      await portal.setTarget(target);
    } catch (reason) {
      setError(failureOf(reason).message);
    } finally {
      setPending(null);
    }
  }

  let status: string;
  let detail: string | null = null;
  let attention = false;
  if (!portal.baseUrl) {
    status =
      flavor === 'full'
        ? 'Waiting for the web portal on this TV…'
        : 'Add the Prysm address to control its lights from here.';
  } else if (!portal.supported) {
    status = 'This TV’s browser is too old to control Prysm from the app.';
    detail = `Open ${flavor === 'full' ? (portalAddress ?? portal.baseUrl) : portal.baseUrl} on your phone or computer instead.`;
    attention = true;
  } else if (snapshot) {
    status = describeLighting(snapshot);
    if (snapshot.input && snapshot.session.state === 'running')
      detail = snapshot.input.receiving
        ? `Picture arriving at ${snapshot.input.fps.toFixed(1)} frames per second.`
        : 'No picture arriving yet.';
    if (flavor === 'slim') {
      const kind = (snapshot.settings.capture as { kind?: string } | undefined)?.kind;
      if (kind !== 'remote') {
        detail = `This Prysm takes its picture from ${kind ? (captureNames[kind] ?? kind) : 'nothing yet'}. Choose Remote capture in its portal to use this TV.`;
        attention = true;
      }
    }
  } else if (portal.reach === 'refused') {
    status = `${capitalize(where)} does not allow control from the TV.`;
    detail =
      flavor === 'slim'
        ? "Start Prysm on your computer with --allow-origin null to use these buttons. Capture works without it."
        : 'Restart the TV app; if this persists, check the Prysm service.';
    attention = true;
  } else if (connection?.error) {
    status = `Cannot reach ${where}.`;
    detail =
      portal.reach === 'unreachable'
        ? 'Check that Prysm is running and that the address is right.'
        : connection.error;
    attention = true;
  } else status = `Connecting to ${where}…`;

  const offline = !snapshot || pending !== null;
  const faulted = snapshot?.session.state === 'faulted';
  const effects = snapshot ? effectsOf(snapshot) : null;
  const hasOutput = !!snapshot?.settings.output;
  const hasCapture = !!snapshot?.settings.capture;
  const modes: { value: LightingTarget; label: string; disabled: boolean }[] = [
    { value: 'processed', label: 'Screen sync', disabled: !hasOutput || !hasCapture },
    { value: 'device_native', label: effects ?? 'Light effects', disabled: !effects },
    { value: 'off', label: 'Off', disabled: false },
  ];
  return (
    <section
      className={`card lighting${attention ? ' attention' : ''}`}
      aria-labelledby="lighting-title"
    >
      <h2 id="lighting-title" className="card-title">
        {flavor === 'full' ? 'Lights' : 'Lights on your computer'}
      </h2>
      <p id="lighting-status" className="lighting-status" role="status" aria-live="polite">
        {status}
      </p>
      {detail ? (
        <p id="lighting-detail" className="lighting-detail">
          {detail}
        </p>
      ) : null}
      <div className="modes" data-row="lighting" role="group" aria-label="Lighting mode" hidden={!snapshot}>
        {modes.map((mode) => (
          <button
            key={mode.value}
            type="button"
            id={`mode-${mode.value}`}
            className="mode"
            aria-pressed={snapshot?.target === mode.value}
            disabled={offline || faulted || mode.disabled}
            onClick={() => void choose(mode.value)}
          >
            {mode.label}
          </button>
        ))}
      </div>
      {snapshot && !hasOutput ? (
        <p className="lighting-detail">Set up your lights first{flavor === 'full' ? ' (Lights row).' : ' in Prysm’s portal.'}</p>
      ) : null}
      {snapshot?.failure ? (
        <p className="notice error" role="alert">
          {snapshot.failure.message}
        </p>
      ) : null}
      {error && error !== snapshot?.failure?.message ? (
        <p className="notice error" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}

const capitalize = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

/* ---- Full package: lights and capture of the TV's own Prysm ---- */

export interface WledSection {
  kind: 'wled';
  address: string;
  leds: { top?: number; right?: number; bottom?: number; left?: number };
}
export interface WebosSection {
  kind: 'webos';
  width?: number;
  height?: number;
  fps?: number;
  include_ui?: boolean;
}
export const webosDefaults = { width: 640, height: 360, fps: 30, include_ui: true };
const edges = ['top', 'right', 'bottom', 'left'] as const;
const edgeNames = { top: 'Top', right: 'Right', bottom: 'Bottom', left: 'Left' };

export function wledOf(snapshot: Snapshot | null): WledSection | null {
  const output = snapshot?.settings.output as WledSection | undefined;
  return output?.kind === 'wled' ? output : null;
}
export function webosOf(snapshot: Snapshot | null): WebosSection | null {
  const capture = snapshot?.settings.capture as WebosSection | undefined;
  return capture?.kind === 'webos' ? capture : null;
}
export const ledTotal = (leds: WledSection['leds']) =>
  edges.reduce((sum, edge) => sum + (leds[edge] ?? 0), 0);

export function describeWled(output: WledSection | null): string {
  return output ? `${output.address} · ${ledTotal(output.leds)} LEDs` : 'Add your lights';
}
export function describeCapture(capture: WebosSection | null): string {
  const value = { ...webosDefaults, ...capture };
  return `${value.width} × ${value.height}, ${value.fps} fps${value.include_ui ? ', with menus' : ''}`;
}

const STALE = 'These settings were changed elsewhere. Review them and save again.';

/**
 * Explicit save of one section against the state the dialog started from: a
 * section changed elsewhere (or `stale_revision`) reloads instead of overwriting.
 */
function useSectionSave<S>(
  portal: PortalView,
  read: (snapshot: Snapshot | null) => S | null,
  send: (expected: number, value: S) => Promise<unknown>,
) {
  const [base, setBase] = useState(() => read(portal.latest()));
  const [error, setError] = useState<{ message: string; field?: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [generation, setGeneration] = useState(0);
  async function save(value: S): Promise<boolean> {
    const latest = portal.latest();
    const current = read(latest);
    if (!latest) {
      setError({ message: 'Prysm is not connected. Try again in a moment.' });
      return false;
    }
    if (!equal(current, base)) {
      setBase(current);
      setGeneration((count) => count + 1);
      setError({ message: STALE });
      return false;
    }
    setSaving(true);
    setError(null);
    try {
      await send(latest.revision, value);
      return true;
    } catch (reason) {
      const failure = failureOf(reason);
      if (failure.kind === 'stale_revision') {
        setBase(read(portal.latest()));
        setGeneration((count) => count + 1);
        setError({ message: STALE });
      } else setError({ message: failure.message, field: failure.field });
      return false;
    } finally {
      setSaving(false);
    }
  }
  return { base, error, saving, save, generation };
}

const toCount = (text: string) => (/^\d{1,5}$/.test(text.trim()) ? Number(text) : NaN);

export function WledDialogBody({
  portal,
  done,
}: {
  portal: PortalView;
  done: (message: string) => void;
}) {
  const section = useSectionSave(portal, wledOf, (expected, value: WledSection) =>
    portal.configureOutput(expected, value),
  );
  return (
    <WledForm
      key={section.generation}
      base={section.base}
      error={section.error}
      saving={section.saving}
      save={async (value) => {
        if (await section.save(value)) done('Lights saved.');
      }}
    />
  );
}

function WledForm({
  base,
  error,
  saving,
  save,
}: {
  base: WledSection | null;
  error: { message: string; field?: string } | null;
  saving: boolean;
  save: (value: WledSection) => Promise<void>;
}) {
  const [address, setAddress] = useState(base?.address ?? '');
  const [counts, setCounts] = useState(() =>
    edges.map((edge) => String(base?.leds[edge] ?? 0)),
  );
  const [local, setLocal] = useState<{ message: string; field: string } | null>(null);
  const shown = local ?? (error ? { message: error.message, field: error.field ?? '' } : null);
  useEffect(() => {
    if (!shown?.field) return;
    document
      .getElementById(shown.field === 'output.address' ? 'wled-address' : 'wled-top')
      ?.focus();
  }, [shown?.field, shown?.message]);

  function submit(event: FormEvent) {
    event.preventDefault();
    const trimmed = address.trim().replace(/^http:\/\//i, '').replace(/\/$/, '');
    if (!trimmed) {
      setLocal({ message: 'Enter the WLED address, such as 192.168.1.50.', field: 'output.address' });
      return;
    }
    const numbers = counts.map(toCount);
    const total = numbers.reduce((sum, value) => sum + value, 0);
    if (numbers.some((value) => !Number.isInteger(value) || value > 65535) || !(total >= 1 && total <= 65535)) {
      setLocal({ message: 'Enter whole numbers up to 65535; use 0 for an edge without LEDs. At least one LED in total.', field: 'output.leds' });
      return;
    }
    setLocal(null);
    const leds = { ...base?.leds };
    edges.forEach((edge, index) => {
      leds[edge] = numbers[index]!;
    });
    void save({ ...(base ?? { kind: 'wled' }), kind: 'wled', address: trimmed, leds });
  }
  const invalid = (field: string) => (shown?.field === field ? true : undefined);
  return (
    <form onSubmit={submit} noValidate>
      <label htmlFor="wled-address">WLED address</label>
      <input
        id="wled-address"
        type="text"
        inputMode="url"
        value={address}
        placeholder="192.168.1.50 or wled.local"
        spellCheck={false}
        autoComplete="off"
        autoCapitalize="off"
        aria-invalid={invalid('output.address')}
        aria-describedby={shown ? 'wled-error' : undefined}
        onChange={(event) => setAddress(event.target.value)}
      />
      <p className="field-help">LEDs on each edge of the screen (0 for none):</p>
      <div className="led-grid">
        {edges.map((edge, index) => (
          <span key={edge} className="led-field">
            <label htmlFor={`wled-${edge}`}>{edgeNames[edge]}</label>
            <input
              id={`wled-${edge}`}
              type="text"
              inputMode="numeric"
              value={counts[index]}
              autoComplete="off"
              aria-invalid={invalid('output.leds')}
              onChange={(event) =>
                setCounts(counts.map((count, at) => (at === index ? event.target.value : count)))
              }
            />
          </span>
        ))}
      </div>
      {shown ? (
        <p id="wled-error" className="notice error" role="alert">
          {shown.message}
        </p>
      ) : null}
      <button id="wled-save" type="submit" className="primary" disabled={saving}>
        {saving ? 'Saving…' : 'Save lights'}
      </button>
    </form>
  );
}

const sizes = [
  [320, 180],
  [480, 270],
  [640, 360],
  [960, 540],
  [1280, 720],
];
const rates = [15, 24, 25, 30, 50, 60];

/** Size, rate and menus choices; used by the full portal capture and slim preferences. */
export function captureChoices(value: { width: number; height: number; fps: number }) {
  const sizeList = sizes.some(([w, h]) => w === value.width && h === value.height)
    ? sizes
    : [...sizes, [value.width, value.height]].sort((a, b) => a[0]! - b[0]!);
  const rateList = rates.indexOf(value.fps) >= 0 ? rates : [...rates, value.fps].sort((a, b) => a - b);
  return { sizes: sizeList, rates: rateList };
}

export function CaptureDialogBody({
  portal,
  done,
}: {
  portal: PortalView;
  done: (message: string) => void;
}) {
  const section = useSectionSave(portal, webosOf, (expected, value: WebosSection) =>
    portal.configureCapture(expected, value),
  );
  return (
    <CaptureForm
      key={section.generation}
      base={section.base}
      error={section.error}
      saving={section.saving}
      save={async (value) => {
        if (await section.save(value)) done('Capture settings saved.');
      }}
    />
  );
}

function CaptureForm({
  base,
  error,
  saving,
  save,
}: {
  base: WebosSection | null;
  error: { message: string } | null;
  saving: boolean;
  save: (value: WebosSection) => Promise<void>;
}) {
  const [value, setValue] = useState({ ...webosDefaults, ...base });
  const choices = captureChoices(value);
  return (
    <div>
      <p className="field-help">Picture size</p>
      <div className="choice-row" data-row="size">
        {choices.sizes.map(([width, height]) => (
          <button
            key={`${width}x${height}`}
            type="button"
            className="pill"
            aria-pressed={value.width === width && value.height === height}
            onClick={() => setValue({ ...value, width: width!, height: height! })}
          >
            {width} × {height}
          </button>
        ))}
      </div>
      <p className="field-help">Frame rate</p>
      <div className="choice-row" data-row="rate">
        {choices.rates.map((fps) => (
          <button
            key={fps}
            type="button"
            className="pill"
            aria-pressed={value.fps === fps}
            onClick={() => setValue({ ...value, fps })}
          >
            {fps} fps
          </button>
        ))}
      </div>
      <p className="field-help">What to capture</p>
      <div className="choice-row" data-row="menus">
        <button
          type="button"
          className="pill"
          aria-pressed={value.include_ui}
          onClick={() => setValue({ ...value, include_ui: true })}
        >
          Video + TV menus
        </button>
        <button
          type="button"
          className="pill"
          aria-pressed={!value.include_ui}
          onClick={() => setValue({ ...value, include_ui: false })}
        >
          Video only
        </button>
      </div>
      {error ? (
        <p className="notice error" role="alert">
          {error.message}
        </p>
      ) : null}
      <button
        id="capture-save"
        type="button"
        className="primary"
        disabled={saving}
        onClick={() => void save({ ...(base ?? {}), ...value, kind: 'webos' })}
      >
        {saving ? 'Saving…' : 'Save capture'}
      </button>
    </div>
  );
}
