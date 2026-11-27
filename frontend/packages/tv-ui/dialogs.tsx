/** The TV app's dialogs: the Prysm address, service options, diagnostics, removal and closing. */
import { useState, type FormEvent, type ReactNode, type Ref } from 'react';
import { diagnostics } from './diagnostics';
import { prysmAddress, shown } from './preferences';
import type { TvServiceView } from './service';

export type DialogId =
  'prysm-address' | 'service' | 'diagnostics' | 'cleanup' | 'exit';

/** One choice in a dialog's list: a name, an optional line of detail. */
function Choice({
  id,
  title,
  detail,
  action,
}: {
  id: string;
  title: string;
  detail: string;
  action: () => void;
}) {
  return (
    <button id={id} type="button" className="choice" onClick={action}>
      <span className="choice-name">{title}</span>
      <span className="choice-detail">{detail}</span>
    </button>
  );
}

/** A text field committed with OK on the keyboard or the button beside Cancel. */
function AddressForm({
  address,
  save,
  close,
}: {
  address: string | null;
  save: (address: string | null) => void;
  close: () => void;
}) {
  const id = 'prysm-address-input';
  const [text, setText] = useState(() => (address ? shown(address) : ''));
  const [error, setError] = useState<string | null>(null);
  function submit(event: FormEvent) {
    event.preventDefault();
    const result = prysmAddress(text);
    if ('error' in result) {
      setError(result.error);
      return;
    }
    save(result.value);
    close();
    setError(null);
  }
  return (
    <form onSubmit={submit} noValidate>
      <label htmlFor={id}>Address</label>
      <input
        id={id}
        type="text"
        inputMode="url"
        value={text}
        placeholder="192.168.1.20"
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
      ) : (
        <p className="notice">{' '}</p>
      )}
      <div className="dialog-actions" data-row="form">
        <button id={`${id}-apply`} type="submit" className="primary">
          Use this address
        </button>
        <button
          id="dialog-back"
          type="button"
          className="tonal"
          onClick={close}
        >
          Cancel
        </button>
      </div>
    </form>
  );
}

export function Dialogs({
  dialog,
  modal,
  tv,
  slim,
  setPrysmAddress,
  openDialog,
  closeDialog,
  dismiss,
  onExit,
}: {
  dialog: DialogId;
  modal: Ref<HTMLDivElement>;
  tv: TvServiceView;
  slim: boolean;
  setPrysmAddress: (address: string | null) => void;
  openDialog: (dialog: DialogId) => void;
  closeDialog: () => void;
  dismiss: () => void;
  onExit: () => void;
}) {
  let title: string;
  let description: string;
  let content: ReactNode;
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
    case 'prysm-address':
      title = 'Prysm address';
      description =
        'The address you open Prysm’s web portal with on your computer, such as 192.168.1.20.';
      content = (
        <AddressForm
          address={tv.draft.prysm_address}
          save={setPrysmAddress}
          close={closeDialog}
        />
      );
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
          {diagnostics(
            tv.link.status === 'missing' ? null : tv.state,
            tv.problem,
          )}
        </pre>
      );
      break;
    case 'cleanup':
      title = 'Remove service setup?';
      description =
        'Capture stops and the service is removed with its autostart and permissions. Your settings are kept; set it up again from Service options.';
      actions = (
        <>
          <button
            id="keep-setup"
            type="button"
            className="tonal"
            onClick={closeDialog}
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
      className={`modal${dialog === 'prysm-address' ? ' top' : ''}`}
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
