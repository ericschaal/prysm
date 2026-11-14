import {
  ApplicationError,
  type Candidate,
  type DiscoveryClient,
  type Failure,
} from '@prysm/application-client';

/**
 * In-memory lookups: answers `section/kind` with the scripted candidates
 * (nothing for others), records each call and can refuse the next one.
 */
export function fakeDiscoveryClient(found: Record<string, Candidate[]> = {}) {
  const calls: string[] = [];
  let refusal: Failure | null = null;
  const client: DiscoveryClient = {
    candidates: (section, kind) => {
      calls.push(`${section}/${kind}`);
      const failure = refusal;
      refusal = null;
      return failure
        ? Promise.reject(new ApplicationError(failure))
        : Promise.resolve(found[`${section}/${kind}`] ?? []);
    },
  };
  return {
    client,
    calls,
    refuseNext: (failure: Failure) => {
      refusal = failure;
    },
  };
}
