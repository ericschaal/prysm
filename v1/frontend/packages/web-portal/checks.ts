/** Why `name` cannot be saved (`ColorProfile.name` rules), or `null`. */
export function nameProblem(name: string, maxChars: number) {
  if (!name) return 'Enter a name.';
  if ([...name].length > maxChars) return `Use at most ${maxChars} characters.`;
  // Rust's `char::is_control`: Unicode Cc.
  if (/\p{Cc}/u.test(name)) return 'Remove control characters.';
  return null;
}

export interface NumberLimits {
  min: number;
  /** Absent when only the number type bounds it. */
  max?: number;
  /** `'any'` allows fractions; an integer step also requires multiples of it from `min`. */
  step: number | 'any';
}

function numberKind(step: NumberLimits['step']) {
  if (step === 'any') return 'a number';
  if (step === 2) return 'an even number';
  if (step > 1) return `a multiple of ${step}`;
  return 'a whole number';
}

/** Why `text` is not a valid number within `limits`, or `null`. */
export function numberProblem(text: string, { min, max, step }: NumberLimits) {
  const value = Number(text);
  const range =
    max === undefined ? `of at least ${min}` : `from ${min} to ${max}`;
  const problem = `Enter ${numberKind(step)} ${range}.`;
  if (!text.trim() || !Number.isFinite(value)) return problem;
  if (value < min || (max !== undefined && value > max)) return problem;
  if (
    step !== 'any' &&
    ((value - min) % step !== 0 || !Number.isInteger(value))
  )
    return problem;
  return null;
}
