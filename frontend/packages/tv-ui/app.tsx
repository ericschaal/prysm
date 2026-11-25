/**
 * The TV app of both packages. The supervisor (Luna) owns capture on this TV;
 * the lights belong to a Prysm reached over HTTP: the full package's own
 * portal, or the computer the slim package forwards to.
 */
import {
  useEffect,
  useEffectEvent,
  useSyncExternalStore,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { fullPortal, type Flavor, type TvService } from '@prysm/webos-platform';
import {
  failureOf,
  lightingSituation,
  plainFailure,
} from '@prysm/application-client';
import { CalibrationScreen, QrCode } from './calibration';
import { Dialogs, type DialogId } from './dialogs';
import {
  calibrationBlocker,
  localPortal,
  retrySeconds,
  shown,
  tvScreen,
  tvStatus,
} from './model';
import { qrCode } from './qr';
import { useRemoteNavigation } from './navigation';
import { msePatternPlayer, type PatternPlayer } from './patterns';
import { LightingCard } from './portal';
import {
  checkPortal,
  usePortal,
  type PortalOptions,
} from './portal-connection';
import { useTvService, type Retry } from './service';
import './tv.css';

const parents: Partial<Record<DialogId, DialogId>> = {
  diagnostics: 'service',
  cleanup: 'service',
};

export interface TvAppProps {
  /** The package this bundle belongs to. */
  flavor: Flavor;
  service: TvService;
  portal: PortalOptions;
  /** Close the app (after the user confirmed). Lighting is left as it is. */
  onExit: () => void;
  /** Plays the calibration patterns; default: the TV's MSE player. */
  patterns?: PatternPlayer;
}

/** Set by `pagehide`, which comes before `document.hidden` says so. */
let pageHidden = false;
/** The page is shown now. */
const pageShown = () => !document.hidden && !pageHidden;

function watchVisibility(changed: () => void) {
  const hide = () => {
    pageHidden = true;
    changed();
  };
  const show = () => {
    pageHidden = false;
    changed();
  };
  document.addEventListener('visibilitychange', show);
  window.addEventListener('pagehide', hide);
  window.addEventListener('pageshow', show);
  // The TV can show the page with no visibility event; focus still arrives.
  window.addEventListener('focus', show);
  return () => {
    document.removeEventListener('visibilitychange', show);
    window.removeEventListener('pagehide', hide);
    window.removeEventListener('pageshow', show);
    window.removeEventListener('focus', show);
  };
}

/**
 * Visible pages hold the subscriptions; hidden ones release them. The TV
 * starts the page hidden and may show it before any listener is attached,
 * so the present state is read, not only later events: React reads it again
 * once subscribed. Missing that left the app on "Connecting…" for ever.
 */
function useVisible() {
  return useSyncExternalStore(watchVisibility, pageShown);
}

/** Milliseconds since `since` (a `Date.now()`), updated every second; 0 for `null`. */
function useElapsed(since: number | null) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (since === null) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [since]);
  return since === null ? 0 : Math.max(0, now - since);
}

/**
 * The time left until a restart, as a bar that empties. Keyed by the wait,
 * it runs on through later states; mounted late (back from a dialog or
 * calibration), it starts where the wait is by now.
 */
function RetryBar({ retry }: { retry: Retry }) {
  const [elapsed] = useState(() => (Date.now() - retry.since) / 1000);
  return (
    <span
      className="retry-bar"
      style={{
        animationDuration: `${retry.seconds}s`,
        animationDelay: `${-elapsed}s`,
      }}
      aria-hidden="true"
    />
  );
}

/** The warning when no portal that allows this app answers at a saved address. */
function addressWarning(reach: 'refused' | 'unreachable') {
  return reach === 'refused'
    ? 'Saved. Prysm answers there but doesn’t take commands from the TV; capture still works. The TV guide shows how to allow it.'
    : 'Saved. No Prysm answers at that address yet. Check that Prysm is running and the address is right.';
}

export function TvApp({
  flavor,
  service,
  portal: portalOptions,
  onExit,
  patterns = msePatternPlayer,
}: TvAppProps) {
  /** The portal address while the calibration screen is open. */
  const [calibrating, setCalibrating] = useState<string | null>(null);
  const visible = useVisible();
  const tv = useTvService(service, visible);
  const [starting, setStarting] = useState(false);
  const refocusRef = useRef(false);
  const rootRef = useRef<HTMLElement>(null);
  const slim = flavor === 'slim';
  const { state, draft, link } = tv;
  const connecting = link.status === 'connecting';
  // The full package's portal on this TV, as its worker reports it.
  const ownPortal = state && fullPortal(state);
  const baseUrl = slim ? draft.prysm_address : localPortal(ownPortal);
  // Its own connection: a lost TV service subscription does not end it.
  const portal = usePortal(baseUrl, portalOptions, visible);
  const { snapshot } = portal;
  const nav = useRemoteNavigation<DialogId>({
    root: rootRef,
    parents,
    onExitRequest: () =>
      calibrating ? leaveCalibration() : nav.openDialog('exit'),
    // Focus is placed once the screen has its controls: the service has
    // answered, and so has the portal (or it failed, or there is none).
    settled:
      !connecting &&
      (!portal.runtime || !!snapshot || !!portal.connection?.error),
  });
  // Capture, lights and the slim stream encoding are set up only in this
  // portal, never in this UI.
  const portalAddress = slim ? draft.prysm_address : ownPortal;
  const elapsed = useElapsed(tv.countdown ? tv.stateAt : null);
  const status = tvStatus(
    link,
    draft,
    snapshot
      ? {
          lights: !!snapshot.settings.output,
          address: portalAddress,
          lighting: lightingSituation(snapshot),
          issue: snapshot.device.issue ?? null,
        }
      : null,
  );
  // Counted from the latest state; its bar from when the wait began.
  const retry =
    tv.countdown && state?.worker.phase === 'retrying' ? state.worker : null;
  const screen = tvScreen({
    flavor,
    status,
    state,
    changing: tv.changing,
    portalAddress,
    retryLeft: retry ? retrySeconds(retry, elapsed) : null,
  });
  const portalQr = useMemo(
    () => (portalAddress ? qrCode(portalAddress) : null),
    [portalAddress],
  );

  const { say } = tv;
  /** The newest address check; an older one's answer no longer decides. */
  const checkRef = useRef(0);

  /**
   * Save a Prysm address (slim) and check it: a warning replaces the notice
   * when no portal that allows this app answers there.
   */
  async function setPrysmAddress(address: string | null) {
    checkRef.current += 1;
    const check = checkRef.current;
    if (!(await tv.set({ prysm_address: address })) || !address) return;
    const reach = await checkPortal(address, portalOptions);
    if (reach !== 'ok' && check === checkRef.current)
      say(addressWarning(reach), true);
  }

  function primary() {
    switch (status.action) {
      case 'setup':
        return void tv.setup();
      case 'retry':
        return void tv.retry();
      case 'disable':
        return void tv.enable(false);
      case 'address':
        return nav.openDialog('prysm-address');
      case 'enable':
        return void tv.enable(true);
    }
  }

  /** Screen sync on (calibration needs it), then the calibration screen. */
  async function startCalibration() {
    setStarting(true);
    try {
      if (portal.snapshot?.target !== 'screen_sync')
        await portal.setTarget('screen_sync');
      // Hidden meanwhile: hiding ends calibration, so it does not open.
      if (pageShown()) setCalibrating(portalAddress);
    } catch (reason) {
      tv.say(
        plainFailure(failureOf(reason), 'screen_sync', 'WLED effects'),
        true,
      );
    } finally {
      setStarting(false);
    }
  }
  /** Back, or the app hidden: the pattern ends; lighting stays as it is. */
  function leaveCalibration() {
    setCalibrating(null);
    refocusRef.current = true;
    // Best effort: the engine also ends calibration 10 minutes after the last pattern.
    portal.endCalibration().catch(() => {});
  }
  // Hiding the app ends calibration, like Back.
  const hidden = useEffectEvent(() => {
    if (calibrating) leaveCalibration();
  });
  useEffect(() => {
    let shown = pageShown();
    return watchVisibility(() => {
      const now = pageShown();
      if (shown && !now) hidden();
      shown = now;
    });
  }, []);
  useLayoutEffect(() => {
    if (calibrating || !refocusRef.current) return;
    refocusRef.current = false;
    document.getElementById('calibrate')?.focus();
  });
  const blocker = calibrationBlocker(
    flavor,
    patterns,
    portal,
    state,
    draft.prysm_address,
  );

  // A change applies at once; while one is sent the others wait (the
  // service runs one command at a time), but nothing dims for it.
  const disabled = link.status !== 'live' && link.status !== 'lost';
  const { setupFirst } = screen;

  if (calibrating)
    return (
      <CalibrationScreen
        root={rootRef}
        player={patterns}
        pattern={portal.snapshot?.calibration?.pattern}
        address={calibrating}
      />
    );
  return (
    // The handlers only follow pointer and focus to support remote navigation.
    <div
      className={`tv${status.ready ? ' ready' : ''}`}
      onMouseOver={nav.onMouseOver}
      onFocus={nav.onFocus}
    >
      <main id="app" ref={rootRef} aria-hidden={nav.dialog ? true : undefined}>
        <header className="header">
          <svg className="logo" viewBox="0 0 32 32" aria-hidden="true">
            <path d="m16 3 13 24H3L16 3Z" />
            <path d="m16 3 4 24M3 27l15-12 11 12" />
          </svg>
          <span className="brand">
            prysm<span className="brand-dot">.</span>
          </span>
          <span className="brand-sub">Ambient light</span>
        </header>
        <div className="layout">
          <div className="column" data-column="status">
            <section
              className={`card overview${screen.attention ? ' attention' : ''}`}
              aria-labelledby="status-title"
            >
              <p className="section-label">
                <span className="status-dot" aria-hidden="true" />
                {slim ? 'Capture on this TV' : 'Prysm on this TV'}
              </p>
              <div role="status" aria-live="polite">
                <h1
                  id="status-title"
                  key={screen.mismatch ? 'mismatch' : screen.title}
                >
                  {screen.title}
                </h1>
              </div>
              <p id="status-description" className="description">
                {screen.description}
              </p>
              {/* Always a line, so the buttons never move. */}
              <p id="status-meta" className="meta">
                {tv.countdown ? (
                  <RetryBar key={tv.countdown.since} retry={tv.countdown} />
                ) : null}
                {screen.meta || '\u00a0'}
              </p>
              {screen.showPrimary ? (
                <button
                  id="primary"
                  type="button"
                  className="primary"
                  disabled={screen.mismatch}
                  aria-busy={screen.waiting || undefined}
                  onClick={screen.waiting ? undefined : primary}
                >
                  {screen.label}
                </button>
              ) : null}
              {/* No task (full; slim in a phase this app cannot read): the
                  button's place, kept empty. */}
              {screen.showPrimary ? null : (
                <div className="primary-slot" aria-hidden="true" />
              )}
            </section>
            {/*
              First run: the lights are set up in the portal, so its QR code
              takes the place of the modes, which could not do anything yet.
            */}
            {setupFirst && portalAddress ? (
              <section className="setup-lights" id="portal-address">
                {portalQr ? (
                  <QrCode modules={portalQr} className="setup-qr" />
                ) : null}
                <p className="setup-text">
                  <span className="setup-step">
                    Scan this code with your phone.
                  </span>
                  <code id="portal-address-text">{shown(portalAddress)}</code>
                  Or type this address in a browser.
                </p>
              </section>
            ) : (
              <LightingCard
                flavor={flavor}
                portal={portal}
                connecting={connecting}
              />
            )}
          </div>
          <div className="column" data-column="settings">
            {screen.portalPanel ? (
              <section
                className={`portal-address${portalAddress ? '' : ' empty'}`}
                id="portal-address"
              >
                <QrCode modules={portalQr} className="portal-qr" />
                <p className="portal-text">
                  <span className="section-label">Web portal</span>
                  <code id="portal-address-text">
                    {portalAddress ? shown(portalAddress) : '\u00a0'}
                  </code>
                  Scan or type it on your phone for lights, colours and more.
                </p>
              </section>
            ) : null}
            <section className="settings" aria-labelledby="settings-title">
              <h2 id="settings-title" className="section-label">
                Settings
              </h2>
              <div className="setting-list">
                {slim ? (
                  <SettingRow
                    id="prysm-address"
                    label="Prysm address"
                    value={
                      draft.prysm_address ? shown(draft.prysm_address) : 'Add'
                    }
                    disabled={disabled}
                    open={() => nav.openDialog('prysm-address')}
                  />
                ) : null}
                <SwitchRow
                  id="autostart"
                  label="Start with TV"
                  checked={draft.autostart}
                  disabled={disabled}
                  toggle={() => void tv.set({ autostart: !draft.autostart })}
                />
                <SettingRow
                  id="calibrate"
                  label="Calibrate colours"
                  value={
                    blocker ??
                    (starting ? 'Starting screen sync…' : 'With your phone')
                  }
                  disabled={!!blocker || starting}
                  open={() => void startCalibration()}
                />
                <SettingRow
                  id="service-options"
                  label="Service options"
                  value=""
                  disabled={false}
                  open={() => nav.openDialog('service')}
                />
              </div>
            </section>
          </div>
        </div>
        <footer className="key-help">
          <span>
            <b>↑ ↓ ← →</b> Move
          </span>
          <span>
            <b>OK</b> Choose
          </span>
          <span>
            <b>Back</b> Close
          </span>
          <p
            id="notice"
            className={`notice${tv.error ? ' error' : ''}`}
            role={tv.error ? 'alert' : 'status'}
            aria-live="polite"
          >
            {tv.notice}
          </p>
        </footer>
      </main>
      {nav.dialog ? (
        <Dialogs
          key={nav.dialog}
          dialog={nav.dialog}
          modal={nav.modal}
          tv={tv}
          slim={slim}
          setPrysmAddress={(address) => void setPrysmAddress(address)}
          closeDialog={nav.closeDialog}
          openDialog={nav.openDialog}
          dismiss={nav.dismiss}
          onExit={onExit}
        />
      ) : null}
    </div>
  );
}

/**
 * A row that opens something (a dialog or the calibration screen): label,
 * its current value, and a chevron, the promise of another screen.
 */
function SettingRow({
  id,
  label,
  value,
  disabled,
  open,
}: {
  id: string;
  label: string;
  value: string;
  disabled: boolean;
  open: () => void;
}) {
  return (
    <button
      id={id}
      type="button"
      className="setting"
      aria-haspopup={id === 'calibrate' ? undefined : 'dialog'}
      disabled={disabled}
      onClick={open}
    >
      <span className="setting-label">{label}</span>
      <span id={`${id}-value`} className="setting-value">
        {value}
      </span>
      <span className="chevron" aria-hidden="true" />
    </button>
  );
}

/** An on/off row: OK flips it and it applies at once; no other screen. */
function SwitchRow({
  id,
  label,
  checked,
  disabled,
  toggle,
}: {
  id: string;
  label: string;
  checked: boolean;
  disabled: boolean;
  toggle: () => void;
}) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      className="setting"
      disabled={disabled}
      onClick={toggle}
    >
      <span className="setting-label">{label}</span>
      <span id={`${id}-value`} className="setting-value">
        {checked ? 'On' : 'Off'}
      </span>
      <span className="switch" aria-hidden="true" />
    </button>
  );
}
