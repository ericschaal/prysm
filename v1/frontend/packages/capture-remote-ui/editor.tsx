import type { RemoteReceiverSettings } from '@prysm/contracts/host-desktop';
import { TvPictureFields } from '@prysm/capture-webos-ui';
import { SelectField, useCheck, type EditorProps } from '@prysm/web-portal';
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
        The TV sends this picture to this computer, at the address you open this
        page with: give the TV that address. {'320\u00a0×\u00a0180'} at 30 fps
        is plenty for lighting. If the TV cannot send it, lighting stops and
        says why; change the encoding or size here and start screen sync again.
      </p>
    </>
  );
}
