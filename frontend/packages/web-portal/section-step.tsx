import { use, useId, useState } from 'react';
import type { FailureSection } from '@prysm/application-client';
import {
  adapterFor,
  createSection,
  offeredKinds,
  type AdapterSection,
  type AdapterUis,
} from './adapters';
import { DeveloperContext } from './developer';
import { FieldMessage } from './fields';
import { useFieldScopeContext } from './form';
import { RowChoice } from './rows';

/**
 * The kind as rows to tick (when more than one kind is offered) and the
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
  section: FailureSection;
  adapters: AdapterUis<S>;
  value: S;
  /** The saved section, offered again when its kind is chosen. */
  base: S | null;
  onChange: (next: S) => void;
}) {
  const scope = useFieldScopeContext();
  const nameId = useId();
  const legendId = useId();
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
        <fieldset
          className="kind-rows"
          role="radiogroup"
          aria-labelledby={legendId}
          data-field={`${section}.kind`}
          tabIndex={-1}
        >
          <legend id={legendId}>
            {section === 'capture' ? 'Capture from' : 'Send colours to'}
          </legend>
          <ul className="rows">
            {kinds.map((kind) => (
              <RowChoice
                key={kind}
                name={nameId}
                value={kind}
                label={adapterFor(adapters, kind)?.label ?? kind}
                checked={kind === value.kind}
                disabled={scope.disabled}
                onChoose={() => switchKind(kind)}
              />
            ))}
          </ul>
          <FieldMessage path={`${section}.kind`} />
        </fieldset>
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
