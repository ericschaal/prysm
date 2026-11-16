import {
  use,
  useEffect,
  useId,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from 'react';
import { nameProblem, numberProblem, type NumberLimits } from './checks';
import { FieldContext, shownError, useCheck } from './form';
import { ChevronIcon } from './rows';

/**
 * The line under a control: its help, or in the same place its problem. Both
 * sit in one grid cell, so the slot is as tall as the taller of the two and
 * nothing below moves when a problem comes or goes. A failed save of this
 * control offers its retry here.
 */
function Note({
  id,
  help,
  helpTone,
  error,
  retry,
}: {
  id: string;
  help?: ReactNode;
  helpTone?: 'warning';
  error?: string;
  retry?: () => void;
}) {
  return (
    <div className="field-note" data-state={error ? 'error' : 'help'}>
      <p className="help" id={`${id}-help`} data-tone={helpTone}>
        {help}
      </p>
      <p className="field-error" id={`${id}-error`}>
        {error}
        {error && retry ? (
          <>
            {' '}
            <button type="button" className="link" onClick={retry}>
              Try again
            </button>
          </>
        ) : null}
      </p>
    </div>
  );
}

/** A field's classes: its own, then the caller's. */
function classes(own: string, extra: string | undefined) {
  return extra ? `${own} ${extra}` : own;
}

function useField(path: string, help?: ReactNode, helpTone?: 'warning') {
  const scope = use(FieldContext);
  const id = useId();
  const [touched, setTouched] = useState(false);
  const error = shownError(scope, path, touched);
  const retry = scope.retry?.path === path ? scope.retry.run : undefined;
  let described: string | undefined;
  if (error) described = `${id}-error`;
  else if (help) described = `${id}-help`;
  return {
    scope,
    id,
    error,
    control: {
      id,
      name: path,
      disabled: scope.disabled,
      'aria-invalid': error ? true : undefined,
      'aria-describedby': described,
      onBlur: () => setTouched(true),
    },
    touch: () => setTouched(true),
    note: (
      <Note
        id={id}
        help={help}
        helpTone={helpTone}
        error={error}
        retry={retry}
      />
    ),
  };
}

/** The problem of a group of inputs, such as `output.leds`, in its own slot. */
export function FieldMessage({
  path,
  touched = false,
}: {
  path: string;
  /** Show the group's own check before a save is tried. */
  touched?: boolean;
}) {
  const error = shownError(use(FieldContext), path, touched);
  return (
    <div className="field-note" data-state={error ? 'error' : 'help'}>
      <p className="field-error" data-field-message={path}>
        {error}
      </p>
    </div>
  );
}

interface FieldProps {
  path: string;
  label: string;
  help?: ReactNode;
  /**
   * `warning`: the help says what is wrong with the value now, in the
   * problem colour, without stopping a save (Prysm has the last word).
   */
  helpTone?: 'warning';
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
  check,
  placeholder,
}: FieldProps & {
  value: string;
  onChange: (value: string) => void;
  required?: boolean;
  /** The text's own problem, in place of the required one. */
  check?: (value: string) => string | null;
  placeholder?: string;
}) {
  const field = useField(path, help);
  let problem: string | null = null;
  if (check) problem = check(value);
  else if (required && !value.trim()) problem = 'Enter a value.';
  useCheck(path, problem);
  return (
    <div className={classes('field', className)}>
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
      {field.note}
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
    <div className={classes('field', className)}>
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
      {field.note}
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
    <div className={classes('field', className)}>
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
      {field.note}
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
  ends,
  onPress,
  onRelease,
}: FieldProps & {
  value: number;
  onChange: (value: number) => void;
  min: number;
  max: number;
  step: number;
  format?: (value: number) => string;
  /**
   * What the minimum and maximum do, shown at the ends in place of the
   * label, which stays the slider's accessible name.
   */
  ends?: readonly [string, string];
  /** The pointer or a key went down on the slider: a change may follow. */
  onPress?: () => void;
  /** It came up, or the slider lost focus or the pointer. */
  onRelease?: () => void;
}) {
  const field = useField(path, help);
  // The release is heard on the window after later renders: call the latest.
  const releaseRef = useRef(onRelease);
  useEffect(() => {
    releaseRef.current = onRelease;
  });
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
    <div className={classes('field range', className)}>
      {ends ? (
        <div className="range-heading range-ends">
          <label htmlFor={field.id} className="visually-hidden">
            {label}
          </label>
          <span aria-hidden="true">{ends[0]}</span>
          <output htmlFor={field.id}>{format(value)}</output>
          <span aria-hidden="true">{ends[1]}</span>
        </div>
      ) : (
        <div className="range-heading">
          <label htmlFor={field.id}>{label}</label>
          <output htmlFor={field.id}>{format(value)}</output>
        </div>
      )}
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
        {...(onPress && onRelease
          ? {
              onPointerDown: () => {
                // The browser drags the thumb itself (capturing the pointer
                // here stops that in Safari), so the release is heard on the
                // window: it may come up anywhere.
                const release = () => {
                  window.removeEventListener('pointerup', release, true);
                  window.removeEventListener('pointercancel', release, true);
                  releaseRef.current?.();
                };
                window.addEventListener('pointerup', release, true);
                window.addEventListener('pointercancel', release, true);
                onPress();
              },
              onKeyDown: onPress,
              onKeyUp: onRelease,
              onBlur: () => {
                field.touch();
                onRelease();
              },
            }
          : {})}
      />
      {field.note}
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
    <div className={classes('field check', className)}>
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
      {field.note}
    </div>
  );
}

export function SelectField<V extends string>({
  path,
  label,
  help,
  helpTone,
  className,
  value,
  options,
  onChange,
}: FieldProps & {
  value: V;
  options: readonly { value: V; label: string }[];
  onChange: (value: V) => void;
}) {
  const field = useField(path, help, helpTone);
  return (
    <div className={classes('field', className)}>
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
      {field.note}
    </div>
  );
}

/**
 * Options most setups never change, closed until opened, and open while a
 * field inside (at `paths`) shows a problem. The fields stay mounted while
 * closed, so their checks still count.
 */
export function Advanced({
  paths,
  children,
}: {
  paths: readonly string[];
  children: ReactNode;
}) {
  const scope = use(FieldContext);
  const id = useId();
  const [opened, setOpened] = useState(false);
  const problem = paths.some((path) => shownError(scope, path, false));
  const open = opened || problem;
  return (
    <div className="advanced">
      <button
        type="button"
        className="row-link row-toggle"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpened(!open)}
      >
        <span className="row-label">Advanced</span>
        <ChevronIcon />
      </button>
      <div id={id} hidden={!open}>
        {children}
      </div>
    </div>
  );
}
