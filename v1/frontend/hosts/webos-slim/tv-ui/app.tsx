import { TvApp, type TvAppProps } from '@prysm/tv-ui';

/** The slim package: capture forwarded to Prysm on a computer, which it controls. */
export function WebosSlimTvApp(props: Omit<TvAppProps, 'flavor'>) {
  return <TvApp flavor="slim" {...props} />;
}
