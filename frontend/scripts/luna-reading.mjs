import { variantTags } from './variant-tags.mjs';
// Older apps can receive tags from newer Luna writers. Keep the Rust source
// schemas for every known variant and admit only unknown strings as fallbacks.
const openTags = {
  Asked: 'step',
  FullWorker: 'phase',
  SlimWorker: 'phase',
  SlimLink: 'state',
};

export function lunaReading(document) {
  const reader = structuredClone(document);
  const { schemas } = reader.components;
  for (const name of [
    'ErrorKind',
    'Power',
    'Video',
    'LightingTarget',
    'Fix',
    'OutputIssue',
    'StreamEncoding',
  ]) {
    delete schemas[name].enum;
  }
  for (const [name, tag] of Object.entries(openTags)) {
    const { oneOf } = schemas[name];
    const known = variantTags(schemas[name], tag);
    oneOf.push({
      type: 'object',
      properties: { [tag]: { type: 'string', not: { enum: known } } },
      required: [tag],
    });
  }
  // LS2 itself reports errors without the service-specific errorKind.
  schemas.ErrorReply.required = schemas.ErrorReply.required.filter(
    (field) => field !== 'errorKind',
  );
  delete schemas.Preferences.additionalProperties;
  return reader;
}
