# Error handling

The runtime has one error type, [`prysm_ports::Error`](../../v1/crates/ports/src/error.rs).
Its variants are what callers match on; never match on message text.

| Variant | Meaning | Portal `kind` |
| --- | --- | --- |
| `Invalid` | A setting failed validation; carries the field path, such as `output.address` | `invalid` (422) |
| `Decode` | Adapter settings under `kind` could not be decoded | `invalid` (422) |
| `Adapter` | A device or adapter failed to open, read, write or idle; `section` says capture or output when known | `adapter` (502) |
| `StaleRevision` | Settings were replaced from an old revision | `stale_revision` (409) |
| `NotProcessed` | A calibration pattern while Screen sync is off | `not_processed` (409) |
| `Faulted` | A session did not finish within its deadline; only shutdown is accepted | `faulted` (503) |
| `Panicked`, `Closed` | A runtime task panicked, or the engine stopped | `internal` (500), `unavailable` (503) |

The full wire mapping is in the
[portal contract](../../v1/surfaces/web-portal/HTTP-CONTRACT.md#failures).

## Rules

- Keep causes. Build adapter errors with `Error::adapter(context, source)`;
  sources are shared (`Arc`) so a failure can be returned and kept in the
  state with its chain. `prysm_ports::chain` joins the chain for people.
  Shared capture crates keep their own typed errors (such as the
  non-exhaustive `CaptureError`) as sources.
- Validate at the edge. Settings types reject unknown fields and out-of-range
  values; validation errors name the field relative to the section, and the
  registry adds the section (`fps` becomes `capture.fps`).
- A source that finishes (`Pull::Ended`, a file at its end) is success:
  lighting stops without a failure and the target becomes `off`.
- A session failure is reported once, as `State.failure`. The target stays as
  requested, so the next settings change retries; nothing reconnects on its
  own.
- An output's `idle` runs on every session exit; a failure there is reported,
  not hidden.
- Surface failures (a portal listener that dies) are recorded in
  `surface_failures`; lighting continues.
- Panics are for broken internal invariants, not for device, network or file
  errors.

When a contract changes, test the variants and that causes stay inspectable
through the callers.
