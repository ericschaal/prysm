# One runtime, plain blocking adapters

Date: 2026-11-12

Concurrency lives in one runtime, [`prysm-runtime`](../../../crates/engine/runtime/src/lib.rs):
one engine task owns settings, target and at most one session; one session
thread owns a source, the processor and an output. Adapters are plain
blocking objects behind one object-safe trait each
([`prysm-ports`](../../../crates/engine/ports/src/lib.rs)): `FrameSource::next_frame(timeout)`,
`LedOutput::write`/`idle`, `SettingsStore`. They hold no channels, mailboxes
or state machines; a push-style backend runs on an owner thread and hands
over its newest frame.

Why: in the earlier design every component was its own actor, adapters
carried most of the complexity, and a host could not choose a source at run
time. Shutdown ordering is the structure of the session function, and a
session that misses its deadline is one `Faulted` state.

Also decided with it:

- Ports are blocking and run on the session thread; an async adapter would
  need a runtime handle of its own.
- Adapter settings go through a registry of kinds, not a per-host enum, so
  a new adapter edits no shared type (at the cost of exhaustive matching).
- An LED-count change is an output change: it restarts the session instead
  of changing the processor's geometry in place.

Given up: per-operation tracking and observation streams that resynchronise;
fine-grained cleanup states (a hung stop is `Faulted` and needs a restart);
static dispatch on control paths (one boxed call per frame is noise next to
capture and processing). Output writes share the session thread, so a
blocking write delays the next frame; DDP over UDP does not block.

Replaces: the one-executable application with its controller, operation IDs
and cleanup-pending states. How it is built: [architecture](../architecture.md).
