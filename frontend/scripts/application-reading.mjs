import { variantTags } from './variant-tags.mjs';
/** Adapt the request contract to the values an already open page can read. */
export function applicationReading(document) {
  const reading = structuredClone(document);
  const schemas = reading.components.schemas;
  const { State, Session, LightingFailure } = schemas;
  const string = { type: 'string' };

  // Older portals did not send control. Newer versions can introduce names
  // the UI already knows how to display without assigning them a meaning.
  State.required = State.required.filter((name) => name !== 'control');
  State.properties.target = string;
  schemas.Input.properties.picture = string;
  delete schemas.Behind.enum;
  delete schemas.Pattern.enum;
  delete schemas.Style.enum;
  schemas.DeviceReport.properties.issue = string;
  schemas.Session = {
    description: Session.description,
    type: 'object',
    required: ['state'],
    properties: {
      state: string,
      failure: {},
      message: {},
    },
  };
  schemas.LightingFailureReading = {
    type: 'object',
    required: LightingFailure.oneOf[0].required,
    properties: {
      ...Object.assign(
        {},
        ...LightingFailure.oneOf.map(({ properties }) => properties),
      ),
      kind: string,
      section: string,
    },
  };
  const adapter = schemas.Failure.oneOf.find(({ properties }) =>
    properties.kind.enum.includes('adapter'),
  );
  adapter.properties.section = string;
  schemas.FutureFailure = futureVariant(
    schemas.Failure,
    'kind',
    {
      message: string,
      section: string,
      field: string,
    },
    ['message'],
  );
  State.properties.surface_failures.items = {
    oneOf: ['Failure', 'FutureFailure'].map((name) => ({
      $ref: `#/components/schemas/${name}`,
    })),
  };
  schemas.FutureModes = futureVariant(schemas.Modes, 'status');
  // The state includes deserialized settings even when applying them fails
  // domain validation. Keep their types so the user can see and repair them.
  const integerBounds = {
    uint8: { minimum: 0, maximum: 255 },
    uint16: { minimum: 0, maximum: 65_535 },
    int16: { minimum: -32_768, maximum: 32_767 },
  };
  const visited = new Set();
  function relaxSettingsLimits(schema) {
    if (visited.has(schema)) return;
    visited.add(schema);
    if (schema.$ref)
      relaxSettingsLimits(schemas[schema.$ref.split('/').at(-1)]);
    for (const limit of [
      'minimum',
      'maximum',
      'minLength',
      'maxLength',
      'maxItems',
      'pattern',
    ])
      delete schema[limit];
    if (Object.hasOwn(integerBounds, schema.format))
      Object.assign(schema, integerBounds[schema.format]);
    for (const property of Object.values(schema.properties ?? {}))
      relaxSettingsLimits(property);
    if (schema.items) relaxSettingsLimits(schema.items);
  }
  relaxSettingsLimits(schemas.Settings);
  return reading;
}

function futureVariant(schema, tag, properties = {}, required = []) {
  const known = variantTags(schema, tag);
  return {
    type: 'object',
    required: [tag, ...required],
    properties: {
      [tag]: { type: 'string', not: { enum: known } },
      ...properties,
    },
  };
}
