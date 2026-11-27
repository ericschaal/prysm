# Error handling

The runtime has one error type, [`prysm_ports::Error`](../../crates/engine/ports/src/error.rs).
Its variants are what callers match on; never match on message text.

| Variant | Meaning | Portal `kind` |
| --- | --- | --- |
| `Invalid` | A setting failed validation; carries the field path, such as `output.address` | `invalid` (422) |
| `Decode` | Adapter settings under `kind` could not be decoded; carries the field path where decoding stopped, such as `capture.fps` (the section alone when the section as a whole is at fault) | `invalid` (422) |
| `UnusableSettings` | The settings store could not be read at start (the file does not decode, has another format, or cannot be read); carries the store's error and its causes, no field. The engine runs unconfigured, reporting it until a save | `invalid`, no `field` (422 for a command it refuses) |
| `Adapter` | A device or adapter failed to open, read, write or idle; `section` says capture or output when known | `adapter` (502) |
| `StaleRevision` | Settings were replaced from an old revision | `stale_revision` (409) |
| `NeedsScreenSync` | A calibration pattern while Screen sync is off | `needs_screen_sync` (409) |
| `Faulted` | A session did not finish within its deadline, or did not confirm its source's release (phase `source_unreleased`); only shutdown is accepted | `faulted` (503) |
| `Panicked`, `Closed` | A runtime task panicked, or the engine stopped | `internal` (500), `unavailable` (503) |

The full wire mapping is in the
[portal contract](../../crates/surfaces/web-portal/HTTP-CONTRACT.md#failures).

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
- Report the typed failure with its chain. Each adapter owns readiness.
  A recoverable condition with a watcher keeps the requested mode waiting.
  A blocker on a required adapter stops automatic starts. A settings change
  or explicit recheck can clear it. A diagnostic on a usable handle can
  coexist with an active session.
- Screen sync needs a ready input and output. Open capture before output.
  Losing either ends the session and attempts to turn the lights off. The
  session owner orders cleanup once and awaits local release before reopening.
  An unconfirmed local release faults the session. A remote power-off timeout
  leaves physical power unknown.
- A stable session that crashes can restart immediately after cleanup.
  A replacement that crashes during initialization stops. On TV the
  supervisor owns worker restart. The worker reports successful initialization
  independently of heartbeats and frame delivery ([TV lifecycle](webos.md)).
- An explicit WLED override detected while streaming selects Effects and
  releases both handles. Off attempts one device command. A late observation
  cannot replay a superseded power command.
- Surface failures (a portal listener that dies) are recorded in
  `surface_failures`; lighting continues.
- A settings store that cannot be read is no reason not to start: the
  engine starts unconfigured and reports the load's error, with its cause,
  as the failure until a save. A save the store refused changes nothing in
  the engine; one it took is installed.
- Panics are for broken internal invariants, not for device, network or file
  errors.

When a contract changes, test the variants and that causes stay inspectable
through the callers.
