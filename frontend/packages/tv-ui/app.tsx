/**
 * The TV app of both packages. The supervisor (Luna) owns capture on this TV;
 * the lights belong to a Prysm reached over HTTP: the full package's own
 * portal, or the computer the slim package forwards to.
 */
import {
  useEffect,
  useEffectEvent,
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

type DialogId =
  'instance' | 'autostart' | 'service' | 'diagnostics' | 'cleanup' | 'exit';
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

/** Visible pages hold the subscriptions; hidden ones release them. */
function useVisible() {
  const [visible, setVisible] = useState(() => !document.hidden);
  useEffect(() => {
    const update = () => setVisible(!document.hidden);
    const hide = () => setVisible(false);
    document.addEventListener('visibilitychange', update);
    window.addEventListener('pagehide', hide);
    window.addEventListener('pageshow', update);
    return () => {
      document.removeEventListener('visibilitychange', update);
      window.removeEventListener('pagehide', hide);
      window.removeEventListener('pageshow', update);
    };
  }, []);
  return visible;
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

/** An address as people read it: no trailing slash. */
const shown = (address: string) => address.replace(/\/+$/, '');

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
    tv.dirty,
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
   * Save; a newly saved Prysm address (slim) is checked, and a warning
   * replaces the notice when no portal that allows this app answers there.
   */
  async function saveChecked(save: () => Promise<boolean>) {
    const instance =
      slim && draft.instance !== tv.saved.instance ? draft.instance : null;
    if (!(await save()) || !instance) return;
    const reach = await checkPortal(instance, portalOptions);
    if (reach !== 'ok') say(instanceWarning(reach), true);
  }

  function primary() {
    switch (status.action) {
      case 'setup':
        return void tv.setup();
      case 'disable':
        return void tv.enable(false);
      case 'address':
        return nav.openDialog('instance');
      case 'save_enable':
        return void saveChecked(tv.saveAndEnable);
      case 'enable':
        return void tv.enable(true);
    }
  }

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

  const disabled = !online || tv.changing;
  const rows: { id: DialogId; label: string; value: string }[] = [];
  if (slim) {
    rows.push({
      id: 'instance',
      label: 'Prysm address',
      value: draft.instance ? shown(draft.instance) : 'Add address',
    });
  }
  rows.push({
    id: 'autostart',
    label: 'Start with TV',
    value: draft.autostart ? 'On' : 'Off',
  });

  let savedLabel = 'On this TV';
  if (state && tv.syncing) savedLabel = 'Saving…';
  else if (state && tv.dirty) savedLabel = 'Unsaved changes';
  else if (state) savedLabel = 'Saved';

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
              <div className="main-actions">
                <button
                  id="primary"
                  type="button"
                  className="primary"
                  disabled={status.action === 'none' || tv.changing || mismatch}
                  onClick={primary}
                >
                  {tv.changing ? 'Please wait…' : status.label}
                </button>
                <button
                  id="service-options"
                  type="button"
                  className="quiet"
                  onClick={() => nav.openDialog('service')}
                >
                  Service options
                </button>
              </div>
            </section>
            <LightingCard flavor={flavor} portal={portal} />
          </div>
          <div className="column" data-column="settings">
            {portalAddress ? (
              <section className="card portal-address" id="portal-address">
                {portalQr ? (
                  <QrCode modules={portalQr} className="portal-qr" />
                ) : null}
                <p className="portal-text">
                  <strong>Web portal</strong>
                  <code id="portal-address-text">{shown(portalAddress)}</code>
                  Scan or type it on your phone for lights, colours and more.
                </p>
              </section>
            ) : null}
            <section className="card settings" aria-labelledby="settings-title">
              <div className="section-heading">
                <h2 id="settings-title">Settings</h2>
                <span
                  id="saved-state"
                  className={tv.dirty ? 'unsaved' : undefined}
                >
                  {savedLabel}
                </span>
                <span className="save-actions" hidden={!tv.dirty}>
                  <button
                    id="save"
                    type="button"
                    className="secondary"
                    disabled={disabled || !tv.dirty}
                    onClick={() => void saveChecked(tv.save)}
                  >
                    Save changes
                  </button>
                  <button
                    id="discard"
                    type="button"
                    className="quiet"
                    disabled={tv.changing || !tv.dirty}
                    onClick={tv.discard}
                  >
                    Discard
                  </button>
                </span>
              </div>
              <div className="setting-list">
                {rows.map((row) => (
                  <SettingRow
                    key={row.id}
                    {...row}
                    disabled={disabled}
                    open={() => nav.openDialog(row.id)}
                  />
                ))}
              </div>
              <div className="setting-list">
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
            <b>Back</b> Return
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
          closeDialog={nav.closeDialog}
          openDialog={nav.openDialog}
          dismiss={nav.dismiss}
          onExit={onExit}
        />
      ) : null}
    </div>
  );
}

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
      aria-haspopup="dialog"
      disabled={disabled}
      onClick={open}
    >
      <span className="setting-label">{label}</span>
      <span className="setting-selection">
        <span id={`${id}-value`} className="setting-value">
          {value}
        </span>
        <span className="chevron" aria-hidden="true">
          ›
        </span>
      </span>
    </button>
  );
}

function Choice({
  id,
  title,
  detail,
  selected,
  danger,
  action,
}: {
  id?: string;
  title: string;
  detail?: string;
  selected?: boolean;
  danger?: boolean;
  action: () => void;
}) {
  return (
    <button
      id={id}
      type="button"
      className={`choice${danger ? ' danger' : ''}`}
      aria-pressed={selected}
      onClick={action}
    >
      <span>
        <span className="choice-name">{title}</span>
        {detail ? <span className="choice-detail">{detail}</span> : null}
      </span>
      {selected ? (
        <span className="check" aria-hidden="true">
          ✓
        </span>
      ) : null}
    </button>
  );
}

/** A text field committed with OK on the keyboard or the button below it. */
function AddressForm({
  id,
  label,
  initial,
  placeholder,
  apply,
  button,
}: {
  id: string;
  label: string;
  initial: string;
  placeholder: string;
  apply: (text: string) => string | null;
  button: string;
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
      {error ? (
        <p id={`${id}-error`} className="notice error" role="alert">
          {error}
        </p>
      ) : null}
      <button id={`${id}-apply`} type="submit" className="choice apply">
        {button}
      </button>
    </form>
  );
}

function Dialogs({
  dialog,
  modal,
  tv,
  slim,
  openDialog,
  closeDialog,
  dismiss,
  onExit,
}: {
  dialog: DialogId;
  modal: Ref<HTMLDivElement>;
  tv: ReturnType<typeof useTvService>;
  slim: boolean;
  openDialog: (dialog: DialogId) => void;
  closeDialog: () => void;
  dismiss: () => void;
  onExit: () => void;
}) {
  const { draft } = tv;
  let title = '';
  let description = '';
  let content: ReactNode = null;
  const choose = (patch: Parameters<typeof tv.edit>[0]) => {
    tv.edit(patch);
    closeDialog();
  };
  switch (dialog) {
    case 'instance':
      title = 'Prysm address';
      description =
        'The address you open Prysm’s web portal with on your computer, such as 192.168.1.20.';
      content = (
        <AddressForm
          id="instance-input"
          label="Address"
          initial={draft.instance ?? ''}
          placeholder="192.168.1.20"
          button="Use this address"
          apply={(text) => {
            const result = instanceAddress(text);
            if ('error' in result) return result.error;
            choose({ instance: result.value });
            return null;
          }}
        />
      );
      break;
    case 'autostart':
      title = 'Start with TV';
      description = slim
        ? 'Start capture automatically when the TV turns on.'
        : 'Start screen sync automatically when the TV turns on.';
      content = (
        <>
          <Choice
            title="Off"
            detail="Start it yourself."
            selected={!draft.autostart}
            action={() => choose({ autostart: false })}
          />
          <Choice
            title="On"
            detail="Start when the TV turns on."
            selected={draft.autostart}
            action={() => choose({ autostart: true })}
          />
        </>
      );
      break;
    case 'service':
      title = 'Service options';
      description = 'Manage the Prysm service on this TV.';
      content = (
        <>
          <Choice
            id="repair"
            title="Set up or repair service"
            detail="Install the background service and capture permissions again (needs the Homebrew Channel)."
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
        <pre className="diagnostics" id="diagnostics-text">
          {diagnostics(tv.online === false ? null : tv.state, tv.problem)}
        </pre>
      );
      break;
    case 'cleanup':
      title = 'Remove service setup?';
      description =
        'Capture stops and the service is removed with its autostart and permissions. Your settings are kept; set it up again from Service options.';
      content = (
        <>
          <Choice
            id="keep-setup"
            title="Keep setup"
            action={() => openDialog('service')}
          />
          <Choice
            id="confirm-cleanup"
            title="Remove setup"
            danger
            action={() => {
              dismiss();
              void tv.uninstall();
            }}
          />
        </>
      );
      break;
    case 'exit':
      title = 'Close Prysm?';
      description = slim
        ? 'Capture keeps its current setting when you close the app.'
        : 'The lights keep their current mode when you close the app.';
      content = (
        <>
          <Choice id="stay" title="Stay here" action={closeDialog} />
          <Choice id="exit" title="Close app" action={onExit} />
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
        <button
          id="dialog-back"
          type="button"
          className="quiet dialog-back"
          onClick={closeDialog}
        >
          Back
        </button>
      </section>
    </div>
  );
}
