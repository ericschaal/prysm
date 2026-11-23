import { captureDefaults } from '@prysm/contracts/constants';
import type { WebosSettings } from '@prysm/contracts/host-webos';
import type { ReactNode } from 'react';
import {
  Advanced,
  CheckboxField,
  PictureChoice,
  useCaptureModes,
  type EditorProps,
  type ModesState,
} from '@prysm/web-portal';
import { includeMenusHelp, type TvPicture } from './settings';

/**
 * Size, rate and “include menus” of a TV's picture, shared by this TV's
 * capture and a capture-only TV's stream request: the sizes and rates as
 * `modes` deliver them (`source` and `unreachable` as `PictureChoice`
 * takes them), after `lead`; “Include menus” and `advanced` (the fields at
 * `advancedPaths`) under Advanced. `defaults` are what Rust uses for an
 * omitted field of that section.
 */
export function TvPictureFields<V extends TvPicture>({
  value,
  defaults,
  onChange,
  modes,
  source,
  unreachable,
  lead,
  advanced,
  advancedPaths = [],
}: EditorProps<V> & {
  defaults: Required<TvPicture>;
  modes: ModesState;
  source: string;
  unreachable: string;
  lead?: ReactNode;
  advanced?: ReactNode;
  advancedPaths?: readonly string[];
}) {
  return (
    <>
      {lead}
      <PictureChoice
        value={value}
        defaults={defaults}
        onChange={onChange}
        modes={modes}
        source={source}
        unreachable={unreachable}
      />
      <Advanced paths={['capture.include_ui', ...advancedPaths]}>
        <CheckboxField
          path="capture.include_ui"
          label="Include TV menus"
          help={includeMenusHelp}
          checked={value.include_ui ?? defaults.include_ui}
          onChange={(include_ui) => onChange({ ...value, include_ui })}
        />
        {advanced}
      </Advanced>
    </>
  );
}

/** This TV's own picture: its driver scales, so the answer is always known. */
const webosQuery = { kind: 'webos' } as const;

export function WebosEditor(props: EditorProps<WebosSettings>) {
  const modes = useCaptureModes(webosQuery);
  return (
    <TvPictureFields
      {...props}
      defaults={captureDefaults.webos}
      modes={modes}
      source="This TV"
      unreachable="This TV can’t say what it captures just now."
    />
  );
}
