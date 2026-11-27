import { useState } from 'react';
import { captureDefaults } from '@prysm/contracts/constants';
import type { RemoteReceiverSettings } from '@prysm/contracts/host-desktop';
import { IncludeTvMenus } from '@prysm/capture-webos-ui';
import {
  Advanced,
  nothingOffered,
  PictureChoice,
  SelectField,
  useEachCaptureModes,
  type EditorProps,
} from '@prysm/web-portal';
import { encodingChoices } from './settings';

const defaults = captureDefaults.remote;
const loading = { state: 'loading' } as const;

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
  const encoding = value.encoding ?? defaults.encoding;
  /** The saved encoding stays listed, and in sight while it is not offered. */
  const [saved] = useState(encoding);
  const choices = encodingChoices.map((choice, index) => {
    const modes = states[index] ?? loading;
    return { ...choice, modes, unavailable: nothingOffered(modes) };
  });
  const selected = choices.find((choice) => choice.value === encoding);
  const inSight = choices.some(
    (choice) => choice.value === saved && choice.unavailable,
  );
  const select = (
    <SelectField
      path="capture.encoding"
      label="How the TV sends the picture"
      value={encoding}
      options={choices.filter(
        (choice) =>
          !choice.unavailable ||
          choice.value === saved ||
          choice.value === encoding,
      )}
      onChange={(next) => onChange({ ...value, encoding: next })}
      help={
        selected?.unavailable
          ? 'This TV can’t send the picture this way.'
          : selected?.help
      }
      helpTone={selected?.unavailable ? 'warning' : undefined}
    />
  );
  return (
    <>
      {inSight ? select : null}
      <PictureChoice
        value={value}
        defaults={defaults}
        onChange={onChange}
        modes={selected?.modes ?? loading}
        source="This TV"
        unreachable="The TV isn’t connected. Give the TV app the address of this page."
      />
      <Advanced paths={['capture.include_ui', 'capture.encoding']}>
        <IncludeTvMenus
          value={value}
          onChange={onChange}
          defaultValue={defaults.include_ui}
        />
        {inSight ? null : select}
      </Advanced>
    </>
  );
}
