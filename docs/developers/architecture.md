# Architecture

```text
video source ──frames──▶ Prysm engine and session ──colours──▶ LED output (WLED)
                                 ▲   │
                       commands  │   │ state
                                 │   ▼
                     surfaces: web portal, terminal, TV link
```

**Concurrency lives in one runtime; adapters are plain blocking objects.**
The rule and its reasons: [simplified runtime](../plans/prysm-v1-simplified-runtime.md),
recorded as a [decision](decisions/2026-11-12-one-runtime-plain-adapters.md).
Reference: [v1 README](../../v1/README.md).

## Owners

| Owner | Where | Owns |
| --- | --- | --- |
| Engine | [`prysm-runtime` engine.rs](../../v1/crates/runtime/src/engine.rs) | One Tokio task and the only mailbox: settings, revision, lighting target, calibration pattern, at most one session. |
| Session | [session.rs](../../v1/crates/runtime/src/session.rs) | One OS thread per lighting run: the source, the processor and the output. |
| Adapters | [`v1/adapters`](../../v1/adapters) | One trait each, no channels or state machines: `FrameSource`, `LedOutput`, `SettingsStore`, `FrameSink`. |
| Surfaces | [`v1/surfaces`](../../v1/surfaces) | Ways in and out: the web portal, the terminal UI, the TV links. Each gets an `EngineHandle` and a `Shutdown`. |
| Hosts | [`v1/hosts`](../../v1/hosts) | Executables: a list of adapters and surfaces, plus process options. |

Ports and settings types are in [`prysm-ports`](../../v1/crates/ports/src/lib.rs).
[`prysm-core`](../../v1/crates/core/src/lib.rs) (frame and colour types, colour
correction) and [`prysm-processor`](../../v1/crates/processor/src/lib.rs) are shared by the
runtime and the adapters; keep application, serialisation and runtime code out
of `prysm-core`.

## Commands and state

- Commands (`SetTarget`, `ReplaceSettings`, `SetCalibration`) are handled one
  at a time, in order, and answer once applied. A command finishes even if its
  caller disconnects.
- State is one watched `Snapshot`: target, session state, settings and
  revision, last failure, input rate and latency, output issue, calibration.
  Observers always see the latest value, never a queue.
- Settings changes are validated, persisted, then applied. Processing-only
  changes reach a running session at the next frame. Capture or output
  changes stop the session and start a new one.
- The revision starts at a random value in each process, so a page holding a
  revision from before a restart gets a conflict.
- Loaded settings that no longer validate are kept and reported as the
  failure; a file that cannot be read or parsed stops start-up.

<a id="processing-and-color"></a>

## Processing and colour

The session thread pulls the newest frame from the source, runs the
`PrysmProcessor` (black-bar detection, sampling, smoothing, shadow lift,
colour correction, brightness) and writes `EdgeColors` to the output. Sources
replace untaken frames instead of queueing them, so latency does not build up.
Processing assumes sRGB; the file source tone-maps HDR before it.

The output is opened at the first frame, not when lighting starts, so a source
without video never takes WLED over. After 3 s without frames the output is
idled to the device's own state and reopened at the next frame. While a
session runs the engine measures, once a second, the frame rate, whether
frames arrive and the mean latency per link (`State.input`).

Preview and LED-colour streams are demand-driven: with no viewer, the session
publishes nothing and nothing is encoded. See
[portal contract](../../v1/surfaces/web-portal/HTTP-CONTRACT.md#preview).

## Shutdown ordering

A session ends in one order on every path, including failures: set the stop
flag, leave the loop, close the source, idle the output, join the thread. The
output's `idle` is the failure compensation: off for `off`, the device's own
effects for `device_native`. After the join no frame is in flight.

The engine joins with a deadline (10 s in the desktop hosts). A session that
does not finish puts the engine in a single `Faulted` state that names the
outstanding step and accepts only shutdown. Native watchdogs stay inside the
capture adapters. Host shutdown stops the session with the output idled off.

## Hosts and their adapters

| Host | Sources | Outputs | Surfaces |
| --- | --- | --- | --- |
| `prysm-headless` | `device`, `file`, `remote` | `wled`, `null` | Web portal |
| `prysm-tui` | `device`, `file`, `remote` | `wled`, `null` | Web portal, terminal |
| `prysm-webos` (full TV worker) | `webos` | `wled` | Web portal, link to the TV supervisor |
| `prysm-webos-slim` (slim TV worker) | TV capture into a `RemoteSender`, no engine | — | Link to the TV supervisor |

The two TV workers run under one supervisor, `prysm-webos-service`; see
[LG TV](webos.md) and [UI and host assemblies](../../v1/UI-ASSEMBLIES.md).

## Workspace map

`v1/crates` (core types, processor, ports, runtime, wire protocol, TV link),
`v1/adapters` (sources, outputs, settings file, H.264, Luna and capture
bindings), `v1/surfaces`,
`v1/hosts`, `v1/tools/export-contracts` (OpenAPI for frontend codegen),
`v1/frontend` (portal and TV bundles), `v1/packaging` (Docker, Debian, IPKs).
Contract ownership: [CONTRACTS.md](../../v1/CONTRACTS.md).
