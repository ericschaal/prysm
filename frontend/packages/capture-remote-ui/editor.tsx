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
    <TvPictureFields
      value={value}
      defaults={remoteDefaults}
      sizeHint={largeStreamHint}
      onChange={onChange}
      advancedPaths={['capture.encoding']}
      advanced={
        <SelectField
          path="capture.encoding"
          label="How the TV sends the picture"
          value={encoding}
          options={encodingChoices}
          onChange={(next) => onChange({ ...value, encoding: next })}
          help={
            encodingChoices.find((choice) => choice.value === encoding)?.help
          }
        />
      }
    />
  );
}
