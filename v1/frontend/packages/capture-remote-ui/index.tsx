import type { RemoteReceiverSettings } from '@prysm/contracts/host-desktop';
import { TextField, type AdapterUi, type EditorProps } from '@prysm/web-portal';

function RemoteEditor({ value, onChange }: EditorProps<RemoteReceiverSettings>) {
  return (
    <TextField
      path="capture.listen"
      label="Listen address"
      required
      placeholder="0.0.0.0:7380"
      value={value.listen}
      onChange={(listen) => onChange({ ...value, listen })}
      help="Address and fixed port this computer listens on for frames from a capture-only TV, such as 0.0.0.0:7380."
    />
  );
}

export const remoteCapture: AdapterUi<RemoteReceiverSettings> = {
  label: 'Capture-only TV (remote)',
  create: () => ({ kind: 'remote', listen: '' }),
  Editor: RemoteEditor,
  normalize: (value) => ({ ...value, listen: value.listen.trim() }),
};
