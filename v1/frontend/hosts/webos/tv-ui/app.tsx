import { TvApp, type TvAppProps } from '@prysm/tv-ui';

/** The full package: capture, processing, WLED and the portal on this TV. */
export function WebosTvApp(props: Omit<TvAppProps, 'flavor'>) {
  return <TvApp flavor="full" {...props} />;
}
