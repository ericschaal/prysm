/** Structural equality, ignoring object key order (the server may reorder) and `undefined` fields. */
export function same(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || !a || !b) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const keys = Object.keys(left).filter((key) => left[key] !== undefined);
  if (
    keys.length !==
    Object.keys(right).filter((key) => right[key] !== undefined).length
  )
    return false;
  return keys.every((key) => same(left[key], right[key]));
}
