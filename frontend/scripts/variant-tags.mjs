/** Rust emits an allOf with a tagged object for variants wrapping another schema. */
export function variantTags(schema, tag) {
  return schema.oneOf.flatMap((variant) => {
    const tagged =
      variant.allOf?.find((part) => part.properties?.[tag]) ?? variant;
    const values = tagged.properties?.[tag]?.enum;
    if (!values?.length)
      throw new Error(
        `Missing discriminant ${tag} in ${JSON.stringify(variant)}`,
      );
    return values;
  });
}
