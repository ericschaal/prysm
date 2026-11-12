/**
 * The TV app of both packages. The supervisor (Luna) owns capture on this TV;
 * the lights belong to a Prysm reached over HTTP: the full package's own
 * portal, or the computer the slim package forwards to.
 */
import { useEffect, useMemo, useState, type FormEvent, type ReactNode, type Ref } from 'react';
import type { Flavor, TvService } from '@prysm/webos-platform';
import {
  derivedReceiver,
  diagnostics,
  hostOf,
  instanceAddress,
  listenPort,
  localPortal,
  receiverAddress,
  tvStatus,
} from './model';
import { useRemoteNavigation } from './navigation';
import {
  CaptureDialogBody,
  LightingCard,
  WledDialogBody,
  captureChoices,
  describeCapture,
  describeWled,
  usePortal,
  webosOf,
  wledOf,
  type PortalOptions,
} from './portal';
import { useTvService } from './service';
import './tv.css';

export type { PortalOptions } from './portal';
export { portalSupported } from './portal';

type DialogId =
  | 'instance'
  | 'receiver'
  | 'size'
  | 'rate'
  | 'source'
  | 'autostart'
  | 'lights'
  | 'capture'
  | 'service'
  | 'diagnostics'
  | 'cleanup'
  | 'exit';
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

export function TvApp({ flavor, service, portal: portalOptions, onExit }: TvAppProps) {
  const visible = useVisible();
  const tv = useTvService(service, visible);
  const nav = useRemoteNavigation<DialogId>({
    parents,
    onExitRequest: () => nav.openDialog('exit'),
  });
  const slim = flavor === 'slim';
  const { state, draft, online } = tv;
  const mismatch = !!state && state.flavor !== flavor;
  const baseUrl = slim ? draft.instance : localPortal(state?.portal);
  const portal = usePortal(online ? baseUrl : null, portalOptions, visible);
  const status = tvStatus(online, state, draft, tv.dirty);

  // Slim: frames go to the Prysm's own receiver port, learned from its settings.
  const port = listenPort(portal.snapshot?.settings.capture);
  const suggestion = slim ? derivedReceiver(draft.instance, port) : null;
  useEffect(() => {
    if (!suggestion || suggestion === draft.receiver || !online) return;
    const instanceHost = draft.instance ? hostOf(draft.instance) : null;
    const receiverHost = draft.receiver?.replace(/:\d+$/, '') ?? null;
    if (draft.receiver && receiverHost !== instanceHost) return;
    tv.edit({ receiver: suggestion });
    tv.say(`Frames will go to ${suggestion}, where this Prysm receives them. Save to use it.`);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tv` changes every render.
  }, [suggestion, draft.receiver, draft.instance, online]);

  function primary() {
    switch (status.action) {
      case 'setup':
        return void tv.setup();
      case 'disable':
        return void tv.enable(false);
      case 'address':
        return nav.openDialog('instance');
      case 'save_enable':
        return void tv.saveAndEnable();
      case 'enable':
        return void tv.enable(true);
    }
  }

  const disabled = !online || tv.changing;
  const rows: { id: DialogId; label: string; value: string; disabled?: boolean }[] = [];
  if (slim) {
    rows.push(
      { id: 'instance', label: 'Prysm address', value: draft.instance ?? 'Add address' },
      { id: 'receiver', label: 'Frames go to', value: draft.receiver ?? 'Not set' },
      {
        id: 'size',
        label: 'Capture size',
        value: `${draft.capture.width} × ${draft.capture.height}`,
      },
      { id: 'rate', label: 'Frame rate', value: `${draft.capture.fps} fps` },
      {
        id: 'source',
        label: 'What to capture',
        value: draft.capture.include_ui ? 'Video + TV menus' : 'Video only',
      },
    );
  }
  rows.push({
    id: 'autostart',
    label: slim ? 'Enable with TV' : 'Start screen sync with TV',
    value: draft.autostart ? 'On' : 'Off',
  });
  const portalRows: typeof rows = slim
    ? []
    : [
        {
          id: 'lights',
          label: 'Lights (WLED)',
          value: portal.snapshot ? describeWled(wledOf(portal.snapshot)) : 'Portal not connected',
          disabled: !portal.snapshot,
        },
        {
          id: 'capture',
          label: 'Capture',
          value: portal.snapshot ? describeCapture(webosOf(portal.snapshot)) : 'Portal not connected',
          disabled: !portal.snapshot,
        },
      ];

  let savedLabel = 'On this TV';
  if (state) savedLabel = tv.syncing ? 'Saving…' : tv.dirty ? 'Unsaved changes' : 'Saved';
  const close = (message: string) => {
    nav.dismiss();
    tv.say(message);
  };

  return (
    // The handlers only follow pointer and focus to support remote navigation.
    // eslint-disable-next-line jsx-a11y-x/no-static-element-interactions
    <div
      className={`tv${status.ready ? ' ready' : ''}`}
      onMouseOver={nav.onMouseOver}
      onFocus={nav.onFocus}
    >
      <main id="app" ref={nav.root as Ref<HTMLElement>} aria-hidden={nav.dialog ? true : undefined}>
        <header className="header">
          <span className="brand">
            prysm<span className="brand-dot">.</span>
          </span>
          <span className="brand-sub">{slim ? 'TV capture' : 'Ambient light'}</span>
        </header>
        <div className="layout">
          <div className="column" data-column="status">
            <section
              className={`card overview${status.attention || mismatch ? ' attention' : ''}`}
              aria-labelledby="status-title"
            >
              <p className="section-label">{slim ? 'Capture' : 'Prysm on this TV'}</p>
              <div role="status" aria-live="polite">
                <h1 id="status-title">
                  {mismatch ? 'This app does not match the installed service' : status.title}
                </h1>
              </div>
              <p id="status-description" className="description">
                {mismatch
                  ? 'Install the Prysm package again so the app and its service come from the same package.'
                  : status.description}
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
            <LightingCard
              flavor={flavor}
              portal={portal}
              instance={draft.instance}
              portalAddress={state?.portal ?? null}
            />
          </div>
          <div className="column" data-column="settings">
            <section className="card settings" aria-labelledby="settings-title">
              <div className="section-heading">
                <h2 id="settings-title">Settings</h2>
                <span id="saved-state" className={tv.dirty ? 'unsaved' : undefined}>
                  {savedLabel}
                </span>
                <span className="save-actions" hidden={!tv.dirty}>
                  <button
                    id="save"
                    type="button"
                    className="secondary"
                    disabled={disabled || !tv.dirty}
                    onClick={() => void tv.save()}
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
              {portalRows.length ? (
                <>
                  <h3 className="subheading">Lights and capture</h3>
                  <div className="setting-list">
                    {portalRows.map((row) => (
                      <SettingRow
                        key={row.id}
                        {...row}
                        disabled={!!row.disabled}
                        open={() => nav.openDialog(row.id)}
                      />
                    ))}
                  </div>
                </>
              ) : null}
              <div className="setting-list">
                <button id="calibrate" type="button" className="setting" disabled>
                  <span className="setting-label">Calibrate colours</span>
                  <span className="setting-value">Not available in this version yet</span>
                </button>
              </div>
              {!slim && state?.portal ? (
                <p className="portal-address" id="portal-address">
                  <strong>Web portal</strong>
                  <code>{state.portal}</code>
                  Open it on your phone or computer for colours, profiles and the preview.
                </p>
              ) : null}
            </section>
          </div>
        </div>
        <p
          id="notice"
          className={`notice${tv.error ? ' error' : ''}`}
          role={tv.error ? 'alert' : 'status'}
          aria-live="polite"
        >
          {tv.notice}
        </p>
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
        </footer>
      </main>
      {nav.dialog ? (
        <Dialogs
          key={nav.dialog}
          dialog={nav.dialog}
          modal={nav.modal}
          tv={tv}
          slim={slim}
          portal={portal}
          closeDialog={nav.closeDialog}
          openDialog={nav.openDialog}
          dismiss={nav.dismiss}
          done={close}
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
  inputMode,
  apply,
  button,
}: {
  id: string;
  label: string;
  initial: string;
  placeholder: string;
  inputMode: 'url' | 'decimal';
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
        inputMode={inputMode}
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
      <button id={`${id}-apply`} type="submit" className="choice">
        <span className="choice-name">{button}</span>
      </button>
    </form>
  );
}

function Dialogs({
  dialog,
  modal,
  tv,
  slim,
  portal,
  openDialog,
  closeDialog,
  dismiss,
  done,
  onExit,
}: {
  dialog: DialogId;
  modal: Ref<HTMLDivElement>;
  tv: ReturnType<typeof useTvService>;
  slim: boolean;
  portal: ReturnType<typeof usePortal>;
  openDialog: (dialog: DialogId) => void;
  closeDialog: () => void;
  dismiss: () => void;
  done: (message: string) => void;
  onExit: () => void;
}) {
  const { draft } = tv;
  const capture = draft.capture;
  const choices = useMemo(() => captureChoices(capture), [capture]);
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
        'The computer running Prysm. A host name or IP uses port 8787; you can also enter the full address you open Prysm with.';
      content = (
        <AddressForm
          id="instance-input"
          label="Address"
          initial={draft.instance ?? ''}
          placeholder="192.168.1.20 or http://prysm.local:8787"
          inputMode="url"
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
    case 'receiver':
      title = 'Frames go to';
      description =
        'The IP address and port where Prysm receives this TV’s picture (its Remote capture “listen” port). Filled in for you when Prysm can be reached.';
      content = (
        <AddressForm
          id="receiver-input"
          label="IP address and port"
          initial={draft.receiver ?? ''}
          placeholder="192.168.1.20:7380"
          inputMode="decimal"
          button="Use this address"
          apply={(text) => {
            const result = receiverAddress(text);
            if ('error' in result) return result.error;
            choose({ receiver: result.value });
            return null;
          }}
        />
      );
      break;
    case 'size':
      title = 'Capture size';
      description = 'Smaller sizes use less network and processing. 320 × 180 is enough for most lights.';
      content = choices.sizes.map(([width, height]) => (
        <Choice
          key={`${width}x${height}`}
          title={`${width} × ${height}`}
          selected={capture.width === width && capture.height === height}
          action={() => choose({ capture: { ...capture, width: width!, height: height! } })}
        />
      ));
      break;
    case 'rate':
      title = 'Frame rate';
      description = 'Pictures per second sent to Prysm.';
      content = choices.rates.map((fps) => (
        <Choice
          key={fps}
          title={`${fps} fps`}
          selected={capture.fps === fps}
          action={() => choose({ capture: { ...capture, fps } })}
        />
      ));
      break;
    case 'source':
      title = 'What to capture';
      description = 'Choose what appears in the captured picture.';
      content = (
        <>
          <Choice
            title="Video + TV menus"
            detail="Include the picture and on-screen menus."
            selected={capture.include_ui}
            action={() => choose({ capture: { ...capture, include_ui: true } })}
          />
          <Choice
            title="Video only"
            detail="Capture the video picture."
            selected={!capture.include_ui}
            action={() => choose({ capture: { ...capture, include_ui: false } })}
          />
        </>
      );
      break;
    case 'autostart':
      title = slim ? 'Enable with TV' : 'Start screen sync with TV';
      description = slim
        ? 'Enable capture automatically when the TV turns on.'
        : 'Start screen sync automatically when Prysm starts on this TV.';
      content = (
        <>
          <Choice title="Off" detail="Start it yourself." selected={!draft.autostart} action={() => choose({ autostart: false })} />
          <Choice title="On" detail="Start when the TV turns on." selected={draft.autostart} action={() => choose({ autostart: true })} />
        </>
      );
      break;
    case 'lights':
      title = 'Lights (WLED)';
      description = 'Where your WLED controller is and how many LEDs each screen edge has. Saved to Prysm on this TV.';
      content = <WledDialogBody portal={portal} done={done} />;
      break;
    case 'capture':
      title = 'Capture';
      description = 'Size and rate of the picture Prysm samples. Saved to Prysm on this TV.';
      content = <CaptureDialogBody portal={portal} done={done} />;
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
          {tv.online === false ? `Service unavailable: ${tv.notice}` : diagnostics(tv.state)}
        </pre>
      );
      break;
    case 'cleanup':
      title = 'Remove service setup?';
      description =
        'Capture stops and the service is removed with its autostart and permissions. Your settings are kept; set it up again from Service options.';
      content = (
        <>
          <Choice id="keep-setup" title="Keep setup" action={() => openDialog('service')} />
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
    <div ref={modal} id="modal" className="modal">
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
        <button id="dialog-back" type="button" className="quiet dialog-back" onClick={closeDialog}>
          Back
        </button>
      </section>
    </div>
  );
}
