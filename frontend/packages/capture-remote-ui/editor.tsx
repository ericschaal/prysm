import { useState } from 'react';
import type {
  RemoteReceiverSettings,
  StreamEncoding,
} from '@prysm/contracts/host-desktop';
import { TvPictureFields } from '@prysm/capture-webos-ui';
import {
  nothingOffered,
  SelectField,
  useEachCaptureModes,
  type EditorProps,
} from '@prysm/web-portal';
import { encodingChoices, remoteDefaults } from './settings';

/** One question per encoding: only the encoding selects the TV's answer. */
const queries = encodingChoices.map(({ value }) => ({
  kind: 'remote' as const,
  encoding: value,
}));

/**
 * The TV's stream: the sizes and rates its offer has for the chosen
 * encoding, and the encodings it offers (all of them until it has said).
 * The encoding is under Advanced, unless the one saved is not offered.
 */
export function RemoteEditor({
  value,
  onChange,
}: EditorProps<RemoteReceiverSettings>) {
  const states = useEachCaptureModes(queries);
  const encoding = value.encoding ?? remoteDefaults.encoding;
  /** The saved encoding stays listed, and in sight while it is not offered. */
  const [saved] = useState(encoding);
  const modesOf = (of: StreamEncoding) =>
    states[encodingChoices.findIndex((choice) => choice.value === of)] ?? {
      state: 'loading' as const,
    };
  const lacks = (of: StreamEncoding) => nothingOffered(modesOf(of));
  const select = (
    <SelectField
      path="capture.encoding"
      label="How the TV sends the picture"
      value={encoding}
      options={encodingChoices.filter(
        (choice) =>
          !lacks(choice.value) ||
          choice.value === saved ||
          choice.value === encoding,
      )}
      onChange={(next) => onChange({ ...value, encoding: next })}
      help={
        lacks(encoding)
          ? 'This TV can’t send the picture this way.'
          : encodingChoices.find((choice) => choice.value === encoding)?.help
      }
      helpTone={lacks(encoding) ? 'warning' : undefined}
    />
  );
  const inSight = lacks(saved);
  return (
    <TvPictureFields
      value={value}
      defaults={remoteDefaults}
      onChange={onChange}
      modes={modesOf(encoding)}
      source="The TV"
      unreachable="The TV isn’t connected. Give the TV app the address of this page."
      lead={inSight ? select : null}
      advancedPaths={['capture.encoding']}
      advanced={inSight ? null : select}
    />
  );
}
