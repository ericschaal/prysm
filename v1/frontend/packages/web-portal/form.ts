import type { Failure } from '@prysm/application-client';
import {
  createContext,
  use,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react';

/** Shown when a save was refused because the settings changed elsewhere. */
export const staleNotice =
  'Changed on another device, so this was not saved. The current settings are shown.';

/** Why a save did not happen, in plain words, for a form's status line. */
export function saveProblem(failure: Failure) {
  switch (failure.kind) {
    case 'invalid':
      return failure.field
        ? 'Not saved. Check the marked field.'
        : 'Not saved: Prysm can’t use these settings.';
    case 'unavailable':
    case 'timeout':
      return 'Not saved: Prysm did not answer. Try again.';
    case 'busy':
      return 'Not saved: Prysm is busy. Try again in a moment.';
    case 'faulted':
      return 'Not saved: Prysm needs a restart.';
    default:
      return 'Not saved: something went wrong. Try again.';
  }
}

/** What a field says when Prysm refused its value. */
export const refusedValue = 'Prysm can’t use this value.';

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
  /** A failed save of the control at `path`, offered again at that control. */
  retry?: { path: string; run: () => void } | null;
}

export const FieldContext = createContext<FieldScope>({
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
  retry: FieldScope['retry'] = null,
): FieldScope {
  const { server, local, report, edited } = errors;
  return useMemo(
    () => ({
      disabled,
      errors: server,
      local,
      attempted,
      report,
      edited,
      retry,
    }),
    [disabled, server, local, attempted, report, edited, retry],
  );
}

/** The error to show for `path`: the server's, else a shown local one. */
export function shownError(scope: FieldScope, path: string, touched: boolean) {
  return (
    scope.errors[path] ??
    (touched || scope.attempted ? scope.local[path] : undefined)
  );
}

export function useFieldScopeContext() {
  return use(FieldContext);
}

/** Report a check that is not tied to one input, such as an LED total. */
export function useCheck(path: string, message: string | null) {
  const { report } = use(FieldContext);
  useEffect(() => {
    report(path, message);
  }, [report, path, message]);
  useEffect(() => () => report(path, null), [report, path]);
}

/** The control for `field` in `form`, if the form shows one. */
function controlOf(form: HTMLElement | null, field: string | undefined) {
  // Field paths are dotted identifiers from the generated settings.
  const path = field && /^[\w.]+$/.test(field) ? field : null;
  if (!form || !path) return null;
  return (
    form.querySelector<HTMLElement>(`[name="${path}"]`) ??
    form.querySelector<HTMLElement>(`[data-field="${path}"]`)
  );
}

/** Whether `form` shows a control for `field`. */
export const showsField = (form: HTMLElement | null, field?: string) =>
  controlOf(form, field) !== null;

/** Focus the control for `field` in `form`, or the form's alert. */
export function focusField(
  form: HTMLElement | null,
  field: string | undefined,
) {
  (
    controlOf(form, field) ??
    form?.querySelector<HTMLElement>('[data-form-alert]')
  )?.focus();
}
