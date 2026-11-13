import {
  use,
  useId,
  useState,
  type CSSProperties,
  type ReactNode,
} from 'react';
import { nameProblem, numberProblem, type NumberLimits } from './checks';
import { FieldContext, shownError, useCheck } from './form';

function useField(path: string, help?: ReactNode) {
  const scope = use(FieldContext);
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
  const error = shownError(use(FieldContext), path, touched);
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
  // The filled part of the track. Set through the CSSOM, which the portal's
  // CSP (`default-src 'self'`) allows; style attributes in markup it refuses.
  const at = (number: number) =>
    max > min
      ? ((Math.min(Math.max(number, min), max) - min) / (max - min)) * 100
      : 0;
  // A signed control fills from its 0, any other from its minimum.
  const origin = min < 0 && max > 0 ? at(0) : 0;
  const fill = {
    '--fill-from': `${Math.min(origin, at(value))}%`,
    '--fill-to': `${Math.max(origin, at(value))}%`,
  } as CSSProperties;
  return (
    <div className={className ? `field range ${className}` : 'field range'}>
      <div className="range-heading">
        <label htmlFor={field.id}>{label}</label>
        <output htmlFor={field.id}>{format(value)}</output>
      </div>
      <input
        {...field.control}
        type="range"
        style={fill}
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
