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
- A session failure is reported once, in the session it explains
  (`State.session.failure`, on `stopped` or `effects`; screen sync and a
  fault carry none). A failure of what a command replaced (the old output of
  a restart, a session that ended as the lights' own effects were asked for)
  is logged at `warn` and is neither published nor the reply. The target
  stays as requested, so the next settings change retries. Apart from the
  wait for the lights below, the engine does not retry on its own; on the
  TV, the full worker re-applies the
  target after a session failure with a backoff
  ([TV lifecycle](webos.md#supervisor-and-worker)).
- An output's `idle` runs on every session exit; a failure there is reported,
  not hidden.
- A WLED colour frame whose send cannot leave now (a full buffer, no route
  to the device) is dropped, and the next frame is the next try; any other
  send error fails the output at once
  ([`classify.rs`](../../crates/adapters/output-wled/src/classify.rs)).
- An output open the lights did not answer (`Escalation::Unanswered`: off,
  or away from the network) does not end the session: it reports the
  device issue `unreachable` and tries the open again at the first frame
  every 5 s, without end, until the lights answer, lighting stops or the
  settings change. It writes no frames meanwhile and holds no output, and,
  as it is no failure, nothing else retries it (the TV's backoff included).
  The portal and the TV app say "Can’t reach the lights".
  Every other failed open ends the session at once. The rule and why it
  converges are `UNANSWERED_INTERVAL` in
  [`leds.rs`](../../crates/engine/runtime/src/leds.rs).
- A held WLED output whose state checks get no answer reports the same
  issue and keeps sending; its checks go on once a second, without backoff.
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
