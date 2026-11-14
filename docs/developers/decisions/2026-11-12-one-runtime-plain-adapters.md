# One runtime, plain blocking adapters

Date: 2026-11-12

Concurrency lives in one runtime, [`prysm-runtime`](../../../crates/runtime/src/lib.rs):
one engine task owns settings, target and at most one session; one session
thread owns a source, the processor and an output. Adapters are plain
blocking objects behind one object-safe trait each
([`prysm-ports`](../../../crates/ports/src/lib.rs)): `FrameSource::next_frame(timeout)`,
`LedOutput::write`/`idle`, `SettingsStore`. They hold no channels, mailboxes
or state machines; a push-style backend runs on an owner thread and hands
over its newest frame.

Why: in the earlier design every component was its own actor, adapters
carried most of the complexity, and a host could not choose a source at run
time. Shutdown ordering is now the structure of the session function, and a
session that misses its deadline is one `Faulted` state.

Replaces: the one-executable application with its controller, operation IDs
and cleanup-pending states. Design:
[simplified runtime](../../plans/prysm-v1-simplified-runtime.md).
