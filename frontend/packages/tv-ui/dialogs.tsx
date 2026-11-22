/** The TV app's dialogs: the Prysm address, service options, diagnostics, removal and closing. */
import { useState, type FormEvent, type ReactNode, type Ref } from 'react';
import { diagnostics, instanceAddress, shown } from './model';
import type { TvServiceView } from './service';

export type DialogId =
  'instance' | 'service' | 'diagnostics' | 'cleanup' | 'exit';

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

export function Dialogs({
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
  tv: TvServiceView;
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
