import { use, useState } from 'react';
import {
  adapterFor,
  createSection,
  offeredKinds,
  type AdapterSection,
  type AdapterUis,
} from './adapters';
import { DeveloperContext } from './developer';
import { SelectField } from './fields';
import { useFieldScopeContext } from './form';

/**
 * The kind selector (when the host registers more than one kind) and the
 * kind's editor. Switching kinds keeps what was typed for the other kinds and
 * drops the server's errors for the section. Used inside a field scope.
 */
export function SectionFields<S extends AdapterSection>({
  section,
  adapters,
  value,
  base,
  onChange,
}: {
  section: 'capture' | 'output';
  adapters: AdapterUis<S>;
  value: S;
  /** The saved section, offered again when its kind is chosen. */
  base: S | null;
  onChange: (next: S) => void;
}) {
  const scope = useFieldScopeContext();
  /** Drafts of other kinds, so switching back keeps what was typed. */
  const [others, setOthers] = useState<Record<string, S>>({});
  const developer = use(DeveloperContext);
  const kinds = offeredKinds(
    adapters,
    developer,
    [value.kind, base?.kind].filter((kind) => kind !== undefined),
  );
  function switchKind(kind: S['kind']) {
    if (kind === value.kind) return;
    setOthers({ ...others, [value.kind]: value });
    scope.edited(section);
    onChange(
      others[kind] ??
        (base?.kind === kind ? base : createSection(adapters, kind)),
    );
  }
  const Editor = adapterFor(adapters, value.kind)?.Editor;
  return (
    <>
      {kinds.length > 1 ? (
        <SelectField
          path={`${section}.kind`}
          label={section === 'capture' ? 'Capture from' : 'Send colours to'}
          value={value.kind}
          options={kinds.map((kind) => ({
            value: kind,
            label: adapterFor(adapters, kind)?.label ?? kind,
          }))}
          onChange={switchKind}
        />
      ) : null}
      {Editor ? (
        <Editor key={value.kind} value={value} onChange={onChange} />
      ) : (
        <p className="notice warning">
          This {section === 'capture' ? 'video source' : 'kind of lights'} can’t
          be changed here.
        </p>
      )}
    </>
  );
}
