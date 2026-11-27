import { captureDefaults } from '@prysm/contracts/constants';
import type { WebosSettings } from '@prysm/contracts/host-webos';
import {
  Advanced,
  CheckboxField,
  PictureChoice,
  useCaptureModes,
  type EditorProps,
} from '@prysm/web-portal';
import { includeMenusHelp, type TvPicture } from './settings';

/** Both capture sources use the TV's saved Include menus setting. */
export function IncludeTvMenus<V extends TvPicture>({
  value,
  onChange,
  defaultValue,
}: EditorProps<V> & { defaultValue: boolean }) {
  return (
    <CheckboxField
      path="capture.include_ui"
      label="Include TV menus"
      help={includeMenusHelp}
      checked={value.include_ui ?? defaultValue}
      onChange={(include_ui) => onChange({ ...value, include_ui })}
    />
  );
}

/** This TV's own picture: its driver scales, so the answer is always known. */
const webosQuery = { kind: 'webos' } as const;

export function WebosEditor(props: EditorProps<WebosSettings>) {
  const modes = useCaptureModes(webosQuery);
  return (
    <>
      <PictureChoice
        {...props}
        defaults={captureDefaults.webos}
        modes={modes}
        source="This TV"
        unreachable="This TV can’t say what it captures just now."
      />
      <Advanced paths={['capture.include_ui']}>
        <IncludeTvMenus
          {...props}
          defaultValue={captureDefaults.webos.include_ui}
        />
      </Advanced>
    </>
  );
}
