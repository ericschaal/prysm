import { useState, type ReactNode, type Ref } from 'react';
import type { Config, Status } from '../service';
import type { Dialog } from '../hooks/useRemoteNavigation';
import ChoiceButton from './ChoiceButton';

type ChoiceKey = 'source' | 'compression' | 'autostart';
type Choice<K extends ChoiceKey> = [Config[K], string, string];
interface Props {
  dialog: Dialog;
  modal: Ref<HTMLDivElement>;
  draft: Config;
  status: Status | null;
  edit: <K extends keyof Config>(key: K, value: Config[K]) => void;
  openDialog: (dialog: Dialog) => void;
  closeDialog: () => void;
  dismiss: () => void;
  setup: () => void;
  cleanup: () => void;
}

export default function CaptureDialog({
  dialog,
  modal,
  draft,
  status,
  edit,
  openDialog,
  closeDialog,
  dismiss,
  setup,
  cleanup,
}: Props) {
  const [receiver, setReceiver] = useState(draft.receiver);
  let dialogTitle = '';
  let dialogDescription = '';
  let content: ReactNode = null;
  function choices<K extends ChoiceKey>(key: K, values: Choice<K>[]) {
    return values.map(([value, name, detail]) => (
      <ChoiceButton
        key={String(value)}
        id={'choice-' + value}
        title={name}
        detail={detail}
        selected={draft[key] === value}
        action={() => {
          edit(key, value);
          closeDialog();
        }}
      />
    ));
  }
  switch (dialog) {
    case 'receiver':
      dialogTitle = 'Prysm receiver';
      dialogDescription =
        'Enter the address of the computer running Prysm, including the port.';
      content = (
        <>
          <label htmlFor="receiver-input">IP address and port</label>
          <input
            id="receiver-input"
            type="text"
            value={receiver}
            onChange={(event) => setReceiver(event.target.value)}
            placeholder="192.168.1.10:19400"
            spellCheck={false}
            autoComplete="off"
            autoCapitalize="off"
          />
          <ChoiceButton
            id="receiver-apply"
            title="Use this address"
            action={() => {
              edit('receiver', receiver.trim());
              closeDialog();
            }}
          />
        </>
      );
      break;
    case 'source':
      dialogTitle = 'What to capture';
      dialogDescription = 'Choose what appears in the captured picture.';
      content = choices('source', [
        [
          'both',
          'Video + TV menus',
          'Include the picture and on-screen menus.',
        ],
        ['video', 'Video only', 'Capture the video picture.'],
      ]);
      break;
    case 'compression': {
      dialogTitle = 'Compression';
      dialogDescription = 'Choose how frames are sent to Prysm.';
      const values: Choice<'compression'>[] = [
        ['lz4', 'LZ4', 'Lossless compression.'],
        ['none', 'None', 'Uncompressed frames. Uses more bandwidth.'],
        [
          'h264-software',
          'H.264 (software)',
          'Smaller frames, encoded by the TV CPU.',
        ],
      ];
      if (status?.capabilities?.hardwareH264 === true)
        values.push([
          'h264-hardware',
          'H.264 (hardware)',
          'Uses the TV encoder, with software fallback.',
        ]);
      content = choices('compression', values);
      break;
    }
    case 'autostart':
      dialogTitle = 'Enable with TV';
      dialogDescription =
        'Start the sender automatically when the TV turns on.';
      content = choices('autostart', [
        [false, 'Off', 'Enable the sender manually from this app.'],
        [true, 'On', 'Enable the sender when the TV boots.'],
      ]);
      break;
    case 'service':
      dialogTitle = 'Service options';
      dialogDescription = 'Manage the background sender on this TV.';
      content = (
        <>
          <ChoiceButton
            id="repair"
            title="Set up or repair service"
            detail="Restore the background service and capture permissions."
            action={() => {
              closeDialog();
              setup();
            }}
          />
          <ChoiceButton
            id="diagnostics"
            title="View diagnostics"
            detail="Connection details and recent messages."
            action={() => openDialog('diagnostics')}
          />
          <ChoiceButton
            id="cleanup"
            title="Remove service setup…"
            detail="Remove autostart and capture permissions."
            action={() => openDialog('cleanup')}
          />
        </>
      );
      break;
    case 'diagnostics':
      dialogTitle = 'Diagnostics';
      dialogDescription = 'Connection details and the latest sender message.';
      content = (
        <pre className="diagnostics">
          {status
            ? `TV power: ${status.power || 'unknown'}
Video source: ${status.videoAvailable === undefined || status.videoAvailable === null ? 'unknown' : status.videoAvailable ? 'available' : 'unavailable'}
Prysm receiver connected: ${!!status.receiverConnected}
Sender enabled: ${status.enabled}
Sender process running: ${status.senderRunning}

${status.error || status.lastLog || 'No errors reported.'}`
            : 'Service unavailable.'}
        </pre>
      );
      break;
    case 'cleanup':
      dialogTitle = 'Remove service setup?';
      dialogDescription =
        'The sender will stop. Autostart and capture permissions will be removed. Your receiver settings are kept.';
      content = (
        <>
          <ChoiceButton
            id="keep-setup"
            title="Keep setup"
            action={() => openDialog('service')}
          />
          <ChoiceButton
            id="confirm-cleanup"
            title="Remove setup"
            danger
            action={() => {
              dismiss();
              cleanup();
            }}
          />
        </>
      );
      break;
    case 'exit':
      dialogTitle = 'Close Prysm Capture?';
      dialogDescription =
        'The sender keeps its current state when you close this app.';
      content = (
        <>
          <ChoiceButton id="stay" title="Stay here" action={closeDialog} />
          <ChoiceButton
            id="exit"
            title="Close app"
            action={() => window.close()}
          />
        </>
      );
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
        <h2 id="dialog-title">{dialogTitle}</h2>
        <p id="dialog-description" className="dialog-description">
          {dialogDescription}
        </p>
        <div id="dialog-body">{content}</div>
        <button
          id="dialog-back"
          className="quiet dialog-back"
          onClick={closeDialog}
        >
          Back
        </button>
      </section>
    </div>
  );
}
