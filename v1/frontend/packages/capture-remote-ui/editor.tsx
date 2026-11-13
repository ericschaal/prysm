import type { RemoteReceiverSettings } from '@prysm/contracts/host-desktop';
import { TvPictureFields } from '@prysm/capture-webos-ui';
import {
  SelectField,
  TextField,
  useCheck,
  type EditorProps,
} from '@prysm/web-portal';
import {
  encodingChoices,
  encodingProblem,
  largeStreamHint,
  remoteDefaults,
} from './settings';

export function RemoteEditor({
  value,
  onChange,
}: EditorProps<RemoteReceiverSettings>) {
  const encoding = value.encoding ?? remoteDefaults.encoding;
  useCheck('capture.encoding', encodingProblem(value));
  return (
    <>
      <TextField
        path="capture.listen"
        label="Listen address"
        required
        placeholder="0.0.0.0:7380"
        value={value.listen}
        onChange={(listen) => onChange({ ...value, listen })}
        help="Address and fixed port this computer listens on for frames from a capture-only TV, such as 0.0.0.0:7380."
      />
      <TvPictureFields
        value={value}
        defaults={remoteDefaults}
        sizeHint={largeStreamHint}
        onChange={onChange}
      />
      <SelectField
        path="capture.encoding"
        label="Stream encoding"
        value={encoding}
        options={encodingChoices}
        onChange={(next) => onChange({ ...value, encoding: next })}
        help={encodingChoices.find((choice) => choice.value === encoding)?.help}
      />
      <p className="help">
        The TV sends this picture to this computer. {'320\u00a0×\u00a0180'} at
        30 fps is plenty for lighting. If the TV cannot send it, lighting stops
        and says why; change the encoding or size here and start screen sync
        again.
      </p>
    </>
  );
}
