import type { WebosSettings } from '@prysm/contracts/host-webos';
import type { SettingsPanelProps } from '@prysm/web-portal';

/** Defaults and limits of the `webos` source; Rust validates again. */
const fields = [
  { name: 'width', label: 'Width', min: 2, max: 1920, step: 2, fallback: 640 },
  { name: 'height', label: 'Height', min: 2, max: 1080, step: 2, fallback: 360 },
  { name: 'fps', label: 'Frames per second', min: 1, max: 60, step: 1, fallback: 30 },
] as const;

/** Edits the TV capture size, rate and menu blending. */
export function WebosCaptureSettings({
  settings,
  disabled,
  onSubmit,
}: SettingsPanelProps<WebosSettings>) {
  return (
    <form
      key={JSON.stringify(settings)}
      onSubmit={(event) => {
        event.preventDefault();
        const form = new FormData(event.currentTarget);
        const number = (name: string) => Number(form.get(name));
        void onSubmit({
          ...settings,
          kind: 'webos',
          width: number('width'),
          height: number('height'),
          fps: number('fps'),
          include_ui: form.get('include_ui') === 'on',
        });
      }}
    >
      <h2>webOS capture</h2>
      <fieldset disabled={disabled}>
        <legend>Capture</legend>
        {fields.map(({ name, label, min, max, step, fallback }) => (
          <label key={name}>
            {label}{' '}
            <input
              name={name}
              type="number"
              min={min}
              max={max}
              step={step}
              required
              defaultValue={settings?.[name] ?? fallback}
            />
          </label>
        ))}
        <label>
          <input
            name="include_ui"
            type="checkbox"
            defaultChecked={settings?.include_ui ?? true}
          />{' '}
          Include TV menus
        </label>
        <button type="submit">Save capture settings</button>
      </fieldset>
    </form>
  );
}
