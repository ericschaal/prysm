import { ApplicationError, type ApplicationClient } from './index';

/** Explicit production stub. It never acknowledges a command it did not dispatch. */
export function unavailableApplicationBridge<C, O>(
  transport: string,
): ApplicationClient<C, O> {
  const unavailable = (): Promise<never> =>
    Promise.reject(
      new ApplicationError({
        kind: 'unavailable',
        message: `${transport} bridge is not implemented.`,
      }),
    );
  return {
    observe: unavailable,
    configureCapture: unavailable,
    configureOutput: unavailable,
    setTarget: unavailable,
    operation: unavailable,
    calibration: {
      open: unavailable,
      feedback: unavailable,
      close: unavailable,
    },
  };
}
