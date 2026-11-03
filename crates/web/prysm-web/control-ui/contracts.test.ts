import type { components } from './api.generated';

type AssertTuple<Expected extends number[], Actual extends Expected> = Actual;

export type CaptureResolutionTuple = AssertTuple<
  [number, number], components['schemas']['CaptureResolution']
>;
export type PreviewColorTuple = AssertTuple<
  [number, number, number], components['schemas']['PreviewColor']
>;
