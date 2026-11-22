# Architecture

```text
video source ──frames──▶ Prysm engine and session ──colours──▶ LED output (WLED)
                                 ▲   │
                       commands  │   │ state
                                 │   ▼
                     surfaces: web portal, terminal, TV link
```

**Concurrency lives in one runtime; adapters are plain blocking objects.**
The rule and its reasons are a
[decision](decisions/2026-11-12-one-runtime-plain-adapters.md).
Reference: [workspace map](workspace.md).

## Owners

| Owner | Where | Owns |
| --- | --- | --- |
| Engine | [`prysm-runtime` engine.rs](../../crates/engine/runtime/src/engine.rs) | One Tokio task and the only mailbox: settings, revision, lighting target, calibration pattern, at most one session. |
| Session | [session.rs](../../crates/engine/runtime/src/session.rs) | One OS thread per lighting run: the source, the processor and the output. |
| Adapters | [`crates/adapters`](../../crates/adapters) | One trait each, no channels or state machines: `FrameSource`, `LedOutput`, `SettingsStore`, `FrameSink`. |
| Surfaces | [`crates/surfaces`](../../crates/surfaces) | Ways in and out: the web portal, the terminal UI, the TV links. Each gets an `EngineHandle` and a `Shutdown`. |
| Hosts | [`crates/hosts`](../../crates/hosts) | Executables: a list of adapters and surfaces, plus process options. |

Ports and settings types are in [`prysm-ports`](../../crates/engine/ports/src/lib.rs).
[`prysm-core`](../../crates/engine/core/src/lib.rs) (frame and colour types, colour
correction) and [`prysm-processor`](../../crates/engine/processor/src/lib.rs) are shared by the
runtime and the adapters; keep application, serialisation and runtime code out
of `prysm-core`.

## Adapters, surfaces and hosts

An adapter is one object behind one trait, with no channels, `select!`,
identities or lifecycle state machine of its own. Opening is construction:
each adapter registers a factory under its kind (`device`, `remote`, `wled`,
...) with typed settings, which it validates itself; the `Registry` erases the
type, so persisted capture and output settings are `{ "kind": "...", ... }`
and the engine and settings file name no adapter; the contract exporter
publishes each adapter's settings schema. A push-style backend (a callback, a
native capture thread) runs on an [`OwnerThread`](../../crates/engine/ports/src/owner.rs),
which keeps only the newest frame for `next_frame`.

A surface (`Surface<H>` in [surface.rs](../../crates/engine/runtime/src/surface.rs))
gets an `EngineHandle` in a full host, or `()` under a
[`Forwarder`](../../crates/engine/runtime/src/forwarder.rs): the session loop
with a `FrameSink` in place of the processor and output, for a capture-only
host such as the slim TV worker. A surface that fails is reported in the
state; lighting goes on without it. A host is a builder list of adapters and
surfaces, so a new capture source, LED driver or TV platform is a new adapter
crate and one builder line.

## Commands and state

- Commands (`SetTarget`, `ReplaceSettings`, `SetCalibration`) are handled one
  at a time, in order, and answer once applied. A command finishes even if its
  caller disconnects.
- State is one watched `Snapshot`: target, session state, settings and
  revision, last failure, input rate and latency, output issue, calibration.
  Observers always see the latest value, never a queue.
- Settings changes are validated, persisted, then applied, without a
  process restart. Processing-only changes reach a running session at the
  next frame. Capture or output changes (an LED count too) stop the session
  and start a new one, the same path as off then on. If applying fails, the
  settings stay saved and the state carries the failure.
- A calibration pattern is not a session and not a setting: it is state
  beside the target, shown only while the target is `processed` (otherwise
  the command fails as `not_processed`). While it shows, the session runs
  without smoothing (and without correction when `uncorrected`). It clears
  when the target leaves `processed`, at shutdown, and after
  `CALIBRATION_TIMEOUT` (10 minutes) without being set again. A processing
  trial nobody tries again or saves ends after `TRIAL_TIMEOUT` (30 s).
- The session only adds to shared counters; the engine samples them once a
  second while a session runs for `input`.
- The revision starts at a random value in each process, so a page holding a
  revision from before a restart gets a conflict.
- Loaded settings that no longer validate are kept and reported as the
  failure. A settings file that cannot be read or decoded (no `format`, a
  newer format, cut-off JSON, a wrong type, over 1 MiB, unreadable) starts
  the host unconfigured, its cause reported as the failure until a save;
  the file is left as it is, and that save keeps it beside the new one
  ([settings file](../../crates/adapters/settings-file/README.md#unreadable-files)).
- A save that the store refuses leaves the engine's settings as they were;
  a save the store took is installed even if the file's directory could not
  be synced, so the engine always holds what the next start loads.

## What a capture delivers

Whoever owns the hardware answers for it, in three tiers:

1. `validate` (no hardware) refuses only what can never work anywhere:
   protocol and format bounds. The settings schema's limits are at least as
   wide as any hardware's answer.
2. `SourceFactory::modes` says what the hardware selected by the settings
   delivers now, as [`Modes`](../../crates/engine/ports/src/modes.rs): ranges where
   it takes any value in them, fixed modes where it offers fixed ones,
   unknown only when it cannot be asked. The portal serves it as
   `POST /api/v1/capture/modes`, off the engine's mailbox.
3. `open` is the authority: each capture adapter checks the settings against
   its known modes there, so the failure names the field and what is
   delivered. The remote source answers from the offer the TV sends with
   every connection attempt, and checks each connection against it.

<a id="processing-and-color"></a>

## Processing and colour

The session thread pulls the newest frame from the source, runs the
`PrysmProcessor` (black-bar detection, sampling, smoothing, colour boost,
dark-scene glow, colour correction, brightness) and writes `EdgeColors` to the output. Sources
replace untaken frames instead of queueing them, so latency does not build up.
Processing assumes sRGB; the file source tone-maps HDR before it. YUYV and
NV12 are sampled in place at full precision; a capture card that names no
YCbCr matrix gets BT.709 from 720 lines up and BT.601 below. A picture above
640x360 is sampled as its copy scaled down to 640x360: it is read on a lattice
of every nth pixel and row, and its grid and its sample counts (the sample
density's samples per 1000 px) are those of the copy, so the cost and the
detail are the same at any capture size. An edge gets at most one sample per
grid cell along it, at most 128 along the longer side. The WLED output
encodes the linear colours for the gamma WLED applies, read from its
configuration (sRGB until read; see [`observe.rs`](../../crates/adapters/output-wled/src/observe.rs)).
Unless the output's `dither` is off, it dithers each LED between the two
light levels WLED can show around the wanted light (its gamma table, then its
brightness), with a fixed threshold per LED place shared by the LED's three
channels, so dim colours average out across the strip without flicker; off,
it rounds to the nearest byte. WLED's current limiter, RGBW auto-white and
colour correction are not modelled
([`dither.rs`](../../crates/adapters/output-wled/src/dither.rs)). The
settings' chosen style is resolved to plain values when `Processing::config`
builds the processor's `Config`; the processor never learns about styles.

One exception to "concurrency lives in one runtime": inside its blocking
call, the processor splits the sampler's independent per-cell and per-light
loops across a small thread pool it owns (half the cores, at most three
threads, never rayon's global pool) and joins them before returning; nothing is shared
between frames, and the colours do not depend on the thread count. On the
TV two threads take a 640x360 frame from 11 to 7 ms
([decision](decisions/2026-11-14-parallel-sampling.md)).

The output is opened at the first frame, not when lighting starts, so a source
without video never takes WLED over. After 3 s without frames the output is
idled to the device's own state and reopened at the next frame. An output
nobody holds (released, or `device_native` without a session) is put in
another idle mode without being opened (`OutputFactory::idle`): WLED is
switched off, or to its effects, without entering live mode first. While a
session runs the engine measures, once a second, the frame rate, whether
frames arrive and the mean latency per link (`State.input`).

Preview and LED-colour streams are demand-driven: with no viewer, the session
publishes nothing and nothing is encoded. See
[portal contract](../../crates/surfaces/web-portal/HTTP-CONTRACT.md#preview).

## Shutdown ordering

A session ends in one order on every path, including failures: set the stop
flag, leave the loop, close the source, idle the output, join the thread. The
output's `idle` is the failure compensation: off for `off`, the device's own
effects for `device_native`. After the join no frame is in flight.

A session that ends on its own (a failure, or a source that ended) idles its
output off, or in a stop's mode once a stop has asked. A request or
shutdown that arrives after its loop ended is handled after that end,
whichever the engine saw first: the same reply, state, failure and saved
settings. A session that ends on its own while a request is stopping it
ended while being stopped: the reply is its failures, then the output's in
the stop's mode. The thread idles in that mode itself when the stop asked
before it idled; otherwise the session sets it after the join through its
own output (not over a source whose release is unconfirmed): a second idle
happens only when the thread idled before the stop asked. An
ended source turns a `processed` target `off`, so lighting does not restart
until asked again.

The engine joins with a deadline (10 s in the desktop hosts), and gives a
late idle after an own end the same deadline. A session that does not finish
puts the engine in a single `Faulted` state that names the outstanding step
and accepts only shutdown; so does a source whose release is not confirmed
(phase `source_unreleased`), since it may still hold its device. Native
watchdogs stay inside the capture adapters. Host shutdown stops the session
with the output idled off; requests still queued behind it are refused.

Commands have no deadline of their own, nor do the portal's replies; each
adapter bounds its blocking calls instead (`COMMAND_LIMIT` in the runtime).
Opening a video device waits up to 10 s for its format query and activation
together, then, if that ran out, up to 3 s for the release. A WLED idle,
name resolution included, has one deadline, `IDLE_LIMIT` (1.5 s); a WLED
open has one, `OPEN_LIMIT` (3 s): resolution and live mode by its first
1.5 s, switching the device off again after a failure by its end. The WLED
state checks never block the session thread: their connect, request and
reply each go on without waiting, at every frame.

A stop waits for the source's release plus at most `STOP_WAIT` (4.5 s) for
WLED: an open in flight and an idle, or two idles (a release to the
device's effects in flight, or, without the release, a session idling
`Off` on its own when the stop asked for its effects: the thread reads the
stop's mode after closing the source). The desktop hosts assert that each
of their sources' release limits plus `STOP_WAIT` fits their 10 s stop
deadline. The TV's does not fit: its release limit is 4 s and its stop
deadline 6 s, and these stops exceed it, so the engine then faults and only
a restart recovers:

- an open in flight that fails: 3 s, then 4 s and 1.5 s, 8.5 s;
- an open in flight that succeeds: 1.5 s + 4 s + 1.5 s, 7 s;
- a release to the device's effects in flight, then a stop `Off`:
  1.5 s + 4 s + 1.5 s, 7 s.

A stop that meets none of them takes up to 4 s + 1.5 s, 5.5 s.

As a backstop, shutdown gives the engine's blocking adapter and settings
calls one shared grace (the stop deadline unless the host sets one; 1 s in
the full TV worker). A call still running then is abandoned on its thread,
and later calls fail without starting. The session still stops in the order
above. Shutdown fails with the abandoned call unless the stop failed itself
or the engine was already faulted, and the process exits without waiting
for the call.

## Hosts and their adapters

| Host | Sources | Outputs | Surfaces |
| --- | --- | --- | --- |
| `prysm-headless` | `device`, `file`, `remote` | `wled`, `null` | Web portal |
| `prysm-tui` | `device`, `file`, `remote` | `wled`, `null` | Web portal, terminal |
| `prysm-webos` (full TV worker) | `webos` | `wled` | Web portal, link to the TV supervisor |
| `prysm-webos-slim` (slim TV worker) | TV capture into a `RemoteSender`, no engine | — | Link to the TV supervisor |

The two TV workers run under one supervisor, `prysm-webos-service`; see
[LG TV](webos.md) and [UI and host assemblies](ui-assemblies.md).

## Workspace map

Every crate is under `crates/`, one folder per layer: `engine` (core types,
processor, ports, runtime, wire protocol, TV link, version), `adapters`
(sources, outputs, settings file, H.264, Luna and capture bindings),
`surfaces`, `hosts`, `tools` (`export-contracts`: OpenAPI for frontend
codegen; `xtask`; `testkit`: what the tests share). Outside it:
`frontend` (portal and TV bundles), `packaging` (Docker, Debian, IPKs).
Contract ownership: [CONTRACTS.md](contracts.md).
