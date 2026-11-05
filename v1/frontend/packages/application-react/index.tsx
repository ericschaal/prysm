import { useDispatch, useSelector } from 'react-redux';
import type { ApplicationRuntime } from '@prysm/application-state';

export function useApplication<C, O>(application: ApplicationRuntime<C, O>) {
  return useSelector(application.selectors.selectConnection);
}

export function useApplicationDispatch<C, O>() {
  return useDispatch<ApplicationRuntime<C, O>['store']['dispatch']>();
}
