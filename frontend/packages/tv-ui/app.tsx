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
  type FormEvent,
  type ReactNode,
  type Ref,
} from 'react';
import type { Flavor, TvService } from '@prysm/webos-platform';
import { failureOf, plainFailure } from '@prysm/application-client';
import { CalibrationScreen, QrCode } from './calibration';
import {
  calibrationBlocker,
  captureRate,
  diagnostics,
  encodingName,
  instanceAddress,
  localPortal,
  retrySeconds,
  shown,
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
import { useTvService } from './service';
import './tv.css';

type DialogId = 'instance' | 'service' | 'diagnostics' | 'cleanup' | 'exit';
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
  return useSyncExternalStore(
    watchVisibility,
    () => !document.hidden && !pageHidden,
  );
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

/** The warning when no portal that allows this app answers at a saved address. */
function instanceWarning(reach: 'refused' | 'unreachable') {
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
  const nav = useRemoteNavigation<DialogId>({
    root: rootRef,
    parents,
    onExitRequest: () =>
      calibrating ? leaveCalibration() : nav.openDialog('exit'),
  });
  const slim = flavor === 'slim';
  const { state, draft, online } = tv;
  const mismatch = !!state && state.flavor !== flavor;
  const baseUrl = slim ? draft.instance : localPortal(state?.portal);
  const portal = usePortal(online ? baseUrl : null, portalOptions, visible);
  // Capture, lights and the slim stream encoding are set up only in this
  // portal, never in this UI.
  const portalAddress = slim ? draft.instance : (state?.portal ?? null);
  const elapsed = useElapsed(
    state?.worker.phase === 'retrying' ? tv.stateAt : null,
  );
  const status = tvStatus(
    online,
    state,
    draft,
    portal.snapshot
      ? { lights: !!portal.snapshot.settings.output, address: portalAddress }
      : null,
  );
  const retry = state?.worker.phase === 'retrying' ? state.worker : null;
  // One quiet line under the detail, always kept: the countdown to a
  // restart, or while frames flow their rate (and the slim encoding).
  let meta = '';
  if (retry) {
    const left = retrySeconds(retry, elapsed);
    meta = left ? `Trying again in ${left} s` : 'Trying again now…';
  } else if (state?.capturing && !mismatch) {
    const rate = captureRate(state);
    meta = [rate, slim && state.encoding ? encodingName(state.encoding) : null]
      .filter(Boolean)
      .join(' · ');
  }
  const portalQr = useMemo(
    () => (portalAddress ? qrCode(portalAddress) : null),
    [portalAddress],
  );

  const { say } = tv;

  /**
   * Save a Prysm address (slim) and check it: a warning replaces the notice
   * when no portal that allows this app answers there.
   */
  async function setInstance(instance: string | null) {
    if (!(await tv.set({ instance })) || !instance) return;
    const reach = await checkPortal(instance, portalOptions);
    if (reach !== 'ok') say(instanceWarning(reach), true);
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
        return nav.openDialog('instance');
      case 'enable':
        return void tv.enable(true);
    }
  }
  // Full: no button while there is nothing for the service to do; the
  // lighting modes are the one way to choose what the lights do. Slim keeps
  // its place while capture stops ("Please wait…").
  const setupFirst = status.setupLights && !mismatch && !!portalAddress;
  const showPrimary =
    status.action !== 'none' || tv.changing || (slim && !!status.label);

  /** Screen sync on (calibration needs it), then the calibration screen. */
  async function startCalibration() {
    setStarting(true);
    try {
      if (portal.snapshot?.target !== 'processed')
        await portal.setTarget('processed');
      setCalibrating(portalAddress);
    } catch (reason) {
      tv.say(
        plainFailure(failureOf(reason), 'processed', 'WLED effects'),
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
    if (!visible) hidden();
  }, [visible]);
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
    draft.instance,
  );

  // A change applies at once; while one is sent the others wait (the
  // service runs one command at a time), but nothing dims for it.
  const disabled = !online;

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
              className={`card overview${status.attention || mismatch ? ' attention' : ''}`}
              aria-labelledby="status-title"
            >
              <p className="section-label">
                <span className="status-dot" aria-hidden="true" />
                {slim ? 'Capture on this TV' : 'Prysm on this TV'}
              </p>
              <div role="status" aria-live="polite">
                <h1
                  id="status-title"
                  key={mismatch ? 'mismatch' : status.title}
                >
                  {mismatch ? 'Reinstall Prysm' : status.title}
                </h1>
              </div>
              <p id="status-description" className="description">
                {mismatch
                  ? 'The app and its TV service don’t match.'
                  : status.description}
              </p>
              {/* Always a line, so the buttons never move. */}
              <p id="status-meta" className="meta">
                {retry ? (
                  <span
                    key={tv.stateAt}
                    className="retry-bar"
                    style={{ animationDuration: `${retry.seconds}s` }}
                    aria-hidden="true"
                  />
                ) : null}
                {meta || '\u00a0'}
              </p>
              {showPrimary ? (
                <button
                  id="primary"
                  type="button"
                  className="primary"
                  disabled={status.action === 'none' || tv.changing || mismatch}
                  onClick={primary}
                >
                  {tv.changing ? 'Please wait…' : status.label}
                </button>
              ) : null}
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
              <LightingCard flavor={flavor} portal={portal} />
            )}
          </div>
          <div className="column" data-column="settings">
            {portalAddress && !setupFirst ? (
              <section className="portal-address" id="portal-address">
                {portalQr ? (
                  <QrCode modules={portalQr} className="portal-qr" />
                ) : null}
                <p className="portal-text">
                  <span className="section-label">Web portal</span>
                  <code id="portal-address-text">{shown(portalAddress)}</code>
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
                    id="instance"
                    label="Prysm address"
                    value={draft.instance ? shown(draft.instance) : 'Add'}
                    disabled={disabled}
                    open={() => nav.openDialog('instance')}
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
          setInstance={(instance) => void setInstance(instance)}
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

/** One choice in a dialog's list: a name, an optional line of detail. */
function Choice({
  id,
  title,
  detail,
  action,
}: {
  id?: string;
  title: string;
  detail?: string;
  action: () => void;
}) {
  return (
    <button id={id} type="button" className="choice" onClick={action}>
      <span className="choice-name">{title}</span>
      {detail ? <span className="choice-detail">{detail}</span> : null}
    </button>
  );
}

/** A text field committed with OK on the keyboard or the button beside Cancel. */
function AddressForm({
  id,
  label,
  initial,
  placeholder,
  apply,
  button,
  cancel,
}: {
  id: string;
  label: string;
  initial: string;
  placeholder: string;
  apply: (text: string) => string | null;
  button: string;
  cancel: () => void;
}) {
  const [text, setText] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  function submit(event: FormEvent) {
    event.preventDefault();
    setError(apply(text));
  }
  return (
    <form onSubmit={submit} noValidate>
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        type="text"
        inputMode="url"
        value={text}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        autoCapitalize="off"
        aria-invalid={error ? true : undefined}
        aria-describedby={error ? `${id}-error` : undefined}
        onChange={(event) => setText(event.target.value)}
      />
      {/* Always a line, so the buttons never move when an error comes. */}
      {error ? (
        <p id={`${id}-error`} className="notice error" role="alert">
          {error}
        </p>
      ) : (
        <p className="notice">{' '}</p>
      )}
      <div className="dialog-actions" data-row="form">
        <button id={`${id}-apply`} type="submit" className="primary">
          {button}
        </button>
        <button
          id="dialog-back"
          type="button"
          className="tonal"
          onClick={cancel}
        >
          Cancel
        </button>
      </div>
    </form>
  );
}

function Dialogs({
  dialog,
  modal,
  tv,
  slim,
  setInstance,
  openDialog,
  closeDialog,
  dismiss,
  onExit,
}: {
  dialog: DialogId;
  modal: Ref<HTMLDivElement>;
  tv: ReturnType<typeof useTvService>;
  slim: boolean;
  setInstance: (instance: string | null) => void;
  openDialog: (dialog: DialogId) => void;
  closeDialog: () => void;
  dismiss: () => void;
  onExit: () => void;
}) {
  let title = '';
  let description = '';
  let content: ReactNode = null;
  /** The buttons along the bottom; a list dialog has Back alone. */
  let actions: ReactNode = (
    <button
      id="dialog-back"
      type="button"
      className="tonal"
      onClick={closeDialog}
    >
      Back
    </button>
  );
  switch (dialog) {
    case 'instance':
      title = 'Prysm address';
      description =
        'The address you open Prysm’s web portal with on your computer, such as 192.168.1.20.';
      content = (
        <AddressForm
          id="instance-input"
          label="Address"
          initial={tv.draft.instance ? shown(tv.draft.instance) : ''}
          placeholder="192.168.1.20"
          button="Use this address"
          cancel={closeDialog}
          apply={(text) => {
            const result = instanceAddress(text);
            if ('error' in result) return result.error;
            setInstance(result.value);
            closeDialog();
            return null;
          }}
        />
      );
      // The form has its own buttons, under the field.
      actions = null;
      break;
    case 'service':
      title = 'Service options';
      description = 'Manage the Prysm service on this TV.';
      content = (
        <>
          <Choice
            id="repair"
            title="Set up or repair service"
            detail="Install the service and permissions again (needs the Homebrew Channel)."
            action={() => {
              dismiss();
              void tv.setup();
            }}
          />
          <Choice
            id="diagnostics"
            title="View diagnostics"
            detail="Service state and recent messages."
            action={() => openDialog('diagnostics')}
          />
          <Choice
            id="cleanup"
            title="Remove service setup…"
            detail="Stop capture and remove autostart and permissions."
            action={() => openDialog('cleanup')}
          />
        </>
      );
      break;
    case 'diagnostics':
      title = 'Diagnostics';
      description = 'Service state and the latest messages.';
      content = (
        <pre className="diagnostics" id="diagnostics-text" data-scroll>
          {diagnostics(tv.online === false ? null : tv.state, tv.problem)}
        </pre>
      );
      break;
    case 'cleanup':
      title = 'Remove service setup?';
      description =
        'Capture stops and the service is removed with its autostart and permissions. Your settings are kept; set it up again from Service options.';
      // Two answers and no third Back button: the Back key keeps the setup.
      actions = (
        <>
          <button
            id="keep-setup"
            type="button"
            className="tonal"
            onClick={() => openDialog('service')}
          >
            Keep setup
          </button>
          <button
            id="confirm-cleanup"
            type="button"
            className="danger"
            onClick={() => {
              dismiss();
              void tv.uninstall();
            }}
          >
            Remove setup
          </button>
        </>
      );
      break;
    case 'exit':
      title = 'Close Prysm?';
      description = slim
        ? 'Capture keeps its current setting when you close the app.'
        : 'The lights keep their current mode when you close the app.';
      actions = (
        <>
          <button
            id="stay"
            type="button"
            className="tonal"
            onClick={closeDialog}
          >
            Stay here
          </button>
          <button id="exit" type="button" className="primary" onClick={onExit}>
            Close app
          </button>
        </>
      );
      break;
  }
  return (
    <div
      ref={modal}
      id="modal"
      className={`modal${dialog === 'instance' ? ' top' : ''}`}
    >
      <section
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="dialog-title"
        aria-describedby="dialog-description"
      >
        <h2 id="dialog-title">{title}</h2>
        <p id="dialog-description" className="dialog-description">
          {description}
        </p>
        <div id="dialog-body">{content}</div>
        {actions ? (
          <div className="dialog-actions" data-row="actions">
            {actions}
          </div>
        ) : null}
      </section>
    </div>
  );
}
