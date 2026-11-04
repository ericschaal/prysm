import { useState, type ReactNode, type Ref } from 'react';
import type { Config, Status } from '../service';
import type { Dialog } from '../hooks/useRemoteNavigation';
import ChoiceButton from './ChoiceButton';
import { embeddedBuild, supportsEmbedded } from '../processingLocation';

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

// Keep an explicit URL (including :80) exactly as entered until native verification.
function instanceAddress(input: string) {
  const value = input.trim();
  if (!value) return null;
  if (value.includes('://')) return value;
  const host = value.replace(/\/$/, '');
  const hasPort = host.includes(':') && !host.endsWith(']');
  return `http://${host}${hasPort ? '' : ':8787'}`;
}

function videoAvailability(available: Status['videoAvailable']) {
  if (available == null) return 'unknown';
  return available ? 'available' : 'unavailable';
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
  const [receiver, setReceiver] = useState(draft.instanceUrl ?? '');
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
    case 'location':
      if (embeddedBuild && supportsEmbedded(status)) {
        dialogTitle = 'Processing location';
        dialogDescription = 'Choose where Prysm processes the TV picture.';
        content = (
          <>
            <ChoiceButton
              id="choice-remote"
              title="Remote instance"
              selected={draft.location !== 'embedded'}
              action={() => {
                edit('location', 'remote');
                closeDialog();
              }}
            />
            <ChoiceButton
              id="choice-embedded"
              title="On this TV (experimental)"
              selected={draft.location === 'embedded'}
              action={() => {
                edit('location', 'embedded');
                closeDialog();
              }}
            />
          </>
        );
      } else dialogTitle = 'Setting unavailable';
      break;
    case 'receiver':
      dialogTitle = 'Prysm instance URL';
      dialogDescription =
        'A LAN hostname or IP uses HTTP on port 8787. You can also enter the full URL you use to open Prysm.';
      content = (
        <>
          <label htmlFor="receiver-input">Prysm address</label>
          <input
            id="receiver-input"
            type="text"
            value={receiver}
            onChange={(event) => setReceiver(event.target.value)}
            placeholder="192.168.1.10 or https://prysm.home"
            spellCheck={false}
            autoComplete="off"
            autoCapitalize="off"
          />
          <ChoiceButton
            id="receiver-apply"
            title="Use this address"
            action={() => {
              edit('instanceUrl', instanceAddress(receiver));
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
      dialogDescription = 'Enable capture automatically when the TV turns on.';
      content = choices('autostart', [
        [false, 'Off', 'Enable capture manually.'],
        [true, 'On', 'Enable capture when the TV turns on.'],
      ]);
      break;
    case 'service':
      dialogTitle = 'Service options';
      dialogDescription = 'Manage capture on this TV.';
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
      dialogDescription = 'Connection details and the latest status message.';
      content = (
        <pre className="diagnostics">
          {status
            ? `TV power: ${status.power || 'unknown'}
Video source: ${videoAvailability(status.videoAvailable)}
${embeddedBuild && supportsEmbedded(status) && status.config.location === 'embedded' ? 'Processing on this TV' : 'Prysm receiver connected: ' + !!status.receiverConnected}
Capture enabled: ${status.enabled}
Background process running: ${status.senderRunning}

${status.error || status.lastLog || 'No errors reported.'}`
            : 'Service unavailable.'}
        </pre>
      );
      break;
    case 'cleanup':
      dialogTitle = 'Remove service setup?';
      dialogDescription =
        'Capture will stop. This removes autostart and capture permissions. Your settings are kept.';
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
      dialogDescription = 'Closing the app keeps your current capture setting.';
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
