import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

/**
 * The form a field belongs to: whether it is editable, the server's errors by
 * dotted path (`output.address`) and where fields report their own checks.
 */
export interface FieldScope {
  disabled: boolean;
  /** The server's errors, shown at once. */
  errors: Readonly<Record<string, string>>;
  /** Fields' own checks, shown once a field is touched or a save was tried. */
  local: Readonly<Record<string, string>>;
  attempted: boolean;
  /** A field's own check failed (`message`) or passes again (`null`). */
  report: (path: string, message: string | null) => void;
  /** The user changed a field; its server error no longer applies. */
  edited: (path: string) => void;
}

const FieldContext = createContext<FieldScope>({
  disabled: false,
  errors: {},
  local: {},
  attempted: true,
  report: () => {},
  edited: () => {},
});

export const FieldScopeProvider = FieldContext.Provider;

/** Errors a form collects from the server and from its fields' own checks. */
export function useFieldErrors() {
  const [server, setServer] = useState<Record<string, string>>({});
  const [local, setLocal] = useState<Record<string, string>>({});
  const report = useCallback((path: string, message: string | null) => {
    setLocal((errors) => {
      if ((errors[path] ?? null) === message) return errors;
      const next = { ...errors };
      if (message === null) delete next[path];
      else next[path] = message;
      return next;
    });
  }, []);
  const edited = useCallback((path: string) => {
    // An edit also answers an error on its group, e.g. `output.leds`, and
    // replacing a group (`processing.profiles`) answers errors inside it.
    const within = (inner: string, outer: string) =>
      inner === outer ||
      inner.startsWith(`${outer}.`) ||
      inner.startsWith(`${outer}[`);
    const covers = (key: string) => within(path, key) || within(key, path);
    setServer((errors) => {
      if (!Object.keys(errors).some(covers)) return errors;
      return Object.fromEntries(
        Object.entries(errors).filter(([key]) => !covers(key)),
      );
    });
  }, []);
  return {
    server,
    local,
    setServer,
    report,
    edited,
    clearServer: useCallback(() => setServer({}), []),
  };
}

export function useFieldScope(
  disabled: boolean,
  errors: ReturnType<typeof useFieldErrors>,
  attempted: boolean,
): FieldScope {
  const { server, local, report, edited } = errors;
  return useMemo(
    () => ({ disabled, errors: server, local, attempted, report, edited }),
    [disabled, server, local, attempted, report, edited],
  );
}

/** The error to show for `path`: the server's, else a shown local one. */
function shownError(scope: FieldScope, path: string, touched: boolean) {
  return (
    scope.errors[path] ??
    (touched || scope.attempted ? scope.local[path] : undefined)
  );
}

export function useFieldScopeContext() {
  return useContext(FieldContext);
}

/** Report a check that is not tied to one input, such as an LED total. */
export function useCheck(path: string, message: string | null) {
  const { report } = useContext(FieldContext);
  useEffect(() => {
    report(path, message);
  }, [report, path, message]);
  useEffect(() => () => report(path, null), [report, path]);
}

function useField(path: string, help?: ReactNode) {
  const scope = useContext(FieldContext);
  const id = useId();
  const [touched, setTouched] = useState(false);
  const error = shownError(scope, path, touched);
  const helpId = help ? `${id}-help` : undefined;
  const errorId = error ? `${id}-error` : undefined;
  return {
    scope,
    id,
    error,
    control: {
      id,
      name: path,
      disabled: scope.disabled,
      'aria-invalid': error ? true : undefined,
      'aria-describedby':
        [helpId, errorId].filter(Boolean).join(' ') || undefined,
      onBlur: () => setTouched(true),
    },
    touch: () => setTouched(true),
    help: help ? (
      <p className="help" id={helpId}>
        {help}
      </p>
    ) : null,
    message: error ? (
      <p className="field-error" id={errorId}>
        {error}
      </p>
    ) : null,
  };
}

/** A message for a group of inputs, such as `output.leds`. */
export function FieldMessage({
  path,
  touched = false,
}: {
  path: string;
  /** Show the group's own check before a save is tried. */
  touched?: boolean;
}) {
  const error = shownError(useContext(FieldContext), path, touched);
  return error ? (
    <p className="field-error" data-field-message={path}>
      {error}
    </p>
  ) : null;
}

interface FieldProps {
  path: string;
  label: string;
  help?: ReactNode;
  className?: string;
}

export function TextField({
  path,
  label,
  help,
  className,
  value,
  onChange,
  required = false,
  placeholder,
}: FieldProps & {
  value: string;
  onChange: (value: string) => void;
  required?: boolean;
  placeholder?: string;
}) {
  const field = useField(path, help);
  const missing = required && !value.trim();
  useCheck(path, missing ? 'Enter a value.' : null);
  return (
    <div className={className ? `field ${className}` : 'field'}>
      <label htmlFor={field.id}>{label}</label>
      <input
        {...field.control}
        type="text"
        value={value}
        required={required}
        placeholder={placeholder}
        autoComplete="off"
        autoCapitalize="off"
        spellCheck={false}
        onChange={(event) => {
          field.scope.edited(path);
          onChange(event.currentTarget.value);
        }}
      />
      {field.help}
      {field.message}
    </div>
  );
}

/** Why `name` cannot be saved (`ColorProfile.name` rules), or `null`. */
export function nameProblem(name: string, maxChars: number) {
  if (!name) return 'Enter a name.';
  if ([...name].length > maxChars) return `Use at most ${maxChars} characters.`;
  // Rust's `char::is_control`: Unicode Cc.
  if (/[\u0000-\u001f\u007f-\u009f]/.test(name)) return 'Remove control characters.';
  return null;
}

/**
 * A name saved as a whole: committed, trimmed, when the field loses focus or
 * on Enter, so a live save never sends half-typed text.
 */
export function NameField({
  path,
  label,
  help,
  className,
  value,
  onChange,
  maxChars,
}: FieldProps & {
  value: string;
  onChange: (value: string) => void;
  maxChars: number;
}) {
  const field = useField(path, help);
  const [text, setText] = useState(value);
  const [shown, setShown] = useState(value);
  /** Only the user's own typing is checked: a saved name is kept as it is. */
  const [typed, setTyped] = useState(false);
  if (shown !== value) {
    setShown(value);
    setText(value);
    setTyped(false);
  }
  const name = text.trim();
  const problem = nameProblem(name, maxChars);
  useCheck(path, typed ? problem : null);
  function commit() {
    if (!typed || problem !== null) return;
    setText(name);
    setTyped(false);
    if (name !== value) {
      setShown(name);
      onChange(name);
    }
  }
  return (
    <div className={className ? `field ${className}` : 'field'}>
      <label htmlFor={field.id}>{label}</label>
      <input
        {...field.control}
        type="text"
        value={text}
        autoComplete="off"
        onChange={(event) => {
          field.scope.edited(path);
          field.touch();
          setTyped(true);
          setText(event.currentTarget.value);
        }}
        onBlur={() => {
          field.touch();
          commit();
        }}
        onKeyDown={(event) => {
          if (event.key !== 'Enter') return;
          event.preventDefault();
          commit();
        }}
      />
      {field.help}
      {field.message}
    </div>
  );
}

export interface NumberLimits {
  min: number;
  /** Absent when only the number type bounds it. */
  max?: number;
  /** `'any'` allows fractions; an integer step also requires multiples of it from `min`. */
  step: number | 'any';
}

/** Why `text` is not a valid number within `limits`, or `null`. */
export function numberProblem(text: string, { min, max, step }: NumberLimits) {
  const value = Number(text);
  const range =
    max === undefined ? `of at least ${min}` : `from ${min} to ${max}`;
  const kind =
    step === 'any'
      ? 'a number'
      : step === 2
        ? 'an even number'
        : step > 1
          ? `a multiple of ${step}`
          : 'a whole number';
  const problem = `Enter ${kind} ${range}.`;
  if (!text.trim() || !Number.isFinite(value)) return problem;
  if (value < min || (max !== undefined && value > max)) return problem;
  if (step !== 'any' && ((value - min) % step !== 0 || !Number.isInteger(value)))
    return problem;
  return null;
}

export function NumberField({
  path,
  label,
  help,
  className,
  value,
  onChange,
  unit,
  ...limits
}: FieldProps &
  NumberLimits & {
    value: number;
    onChange: (value: number) => void;
    unit?: string;
  }) {
  const field = useField(path, help);
  const [text, setText] = useState(String(value));
  const [shown, setShown] = useState(value);
  // Follow a value changed from outside (another client, a reset) unless the
  // text already means it.
  if (shown !== value) {
    setShown(value);
    if (Number(text) !== value || !text.trim()) setText(String(value));
  }
  const problem = numberProblem(text, limits);
  useCheck(path, problem);
  return (
    <div className={className ? `field ${className}` : 'field'}>
      <label htmlFor={field.id}>
        {label}
        {unit ? <span className="unit"> ({unit})</span> : null}
      </label>
      <input
        {...field.control}
        type="number"
        inputMode={limits.step === 'any' ? 'decimal' : 'numeric'}
        min={limits.min}
        max={limits.max}
        step={limits.step}
        value={text}
        onChange={(event) => {
          const next = event.currentTarget.value;
          field.scope.edited(path);
          field.touch();
          setText(next);
          if (numberProblem(next, limits) === null) {
            const number = Number(next);
            setShown(number);
            onChange(number);
          }
        }}
      />
      {field.help}
      {field.message}
    </div>
  );
}

/** A bounded slider with its value shown beside it. */
export function RangeField({
  path,
  label,
  help,
  className,
  value,
  onChange,
  min,
  max,
  step,
  format = String,
}: FieldProps & {
  value: number;
  onChange: (value: number) => void;
  min: number;
  max: number;
  step: number;
  format?: (value: number) => string;
}) {
  const field = useField(path, help);
  return (
    <div className={className ? `field range ${className}` : 'field range'}>
      <div className="range-heading">
        <label htmlFor={field.id}>{label}</label>
        <output htmlFor={field.id}>{format(value)}</output>
      </div>
      <input
        {...field.control}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        aria-valuetext={format(value)}
        onChange={(event) => {
          field.scope.edited(path);
          onChange(Number(event.currentTarget.value));
        }}
      />
      {field.help}
      {field.message}
    </div>
  );
}

export function CheckboxField({
  path,
  label,
  help,
  className,
  checked,
  onChange,
}: FieldProps & { checked: boolean; onChange: (checked: boolean) => void }) {
  const field = useField(path, help);
  return (
    <div className={className ? `field check ${className}` : 'field check'}>
      <input
        {...field.control}
        type="checkbox"
        checked={checked}
        onChange={(event) => {
          field.scope.edited(path);
          onChange(event.currentTarget.checked);
        }}
      />
      <label htmlFor={field.id}>{label}</label>
      {field.help}
      {field.message}
    </div>
  );
}

export function SelectField<V extends string>({
  path,
  label,
  help,
  className,
  value,
  options,
  onChange,
}: FieldProps & {
  value: V;
  options: readonly { value: V; label: string }[];
  onChange: (value: V) => void;
}) {
  const field = useField(path, help);
  return (
    <div className={className ? `field ${className}` : 'field'}>
      <label htmlFor={field.id}>{label}</label>
      <select
        {...field.control}
        value={value}
        onChange={(event) => {
          const next = options.find(
            (option) => option.value === event.currentTarget.value,
          );
          if (!next) return;
          field.scope.edited(path);
          onChange(next.value);
        }}
      >
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      {field.help}
      {field.message}
    </div>
  );
}
