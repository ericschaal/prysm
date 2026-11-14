# Prysm v1 webOS supervisor

Status: revised 2026-11-12 after the user's decisions (beads `prysm-3jx.17`,
hosts half of `prysm-3jx.9`); steps 1–6 of §11 are implemented and tested on
the development host. Replaces the root
Luna supervisor (`apps/prysm-webos-service`) with v1 hosts. No
backward compatibility: settings, Luna names, paths and the worker protocol are
new. **Clean slate:** the old `io.prysm.capture` app, its settings and any
earlier install are assumed removed before a v1 package is installed; nothing
detects, migrates or converges from them. Builds on the [simplified runtime](prysm-v1-simplified-runtime.md).
Paths under `apps/`, `crates/web/`, `xtask/` and the root `packaging/` name the
earlier implementation, removed after the tag `pre-1.0-consolidation`.

**Constraint.** A hung LG capture call is recovered only by killing the process
that made it. Capture therefore runs in a worker process; the process that owns
the Luna registration observes, restarts and kills it.

## 1. Decisions

1. **Two packages, two workers.** `prysm-webos` (TV UI, native capture,
   embedded processing, WLED, web portal) and `prysm-webos-slim` (TV UI, native
   capture, frames to a headless Prysm). No in-app switching; there is no `mode`.
   Both install as the same app (`io.prysm.tv`) with the same file names, so
   installing one replaces the other; the flavor is a build-time fact of the
   binaries and is reported in `State.flavor`.
2. **Portal on the LAN without a token** (`WebPortal::from_listener(listener,
   assets, Access)`). The full worker serves `0.0.0.0:8787`; the TV shows the URL.
3. **Slim streams continuously** while the TV is awake and capture is enabled.
4. **Slim TV UI on par with today's**: it calls the remote headless portal's
   HTTP API directly. The supervisor owns only TV-local things (receiver,
   enabled, autostart, status). Calibration is beads `prysm-3jx.11` (§7).
5. **Defaults accepted:** the full worker re-applies a non-Off target after a
   non-fault session failure (1–30 s backoff); Screen Saver keeps the worker
   with video Unavailable; an unconfirmed source release is a fault — the
   runtime already reports `SessionState::Faulted(ClosingSource)`
   ([engine.rs](../../crates/runtime/src/engine.rs) `UNRELEASED`), so the
   worker exits `FAULTED` and is restarted.

## 2. Inventory of the current supervisor

Sources: `apps/prysm-webos-service/src/supervisor.rs`,
`apps/prysm-webos-service/src/application.rs`,
`apps/prysm-webos-service/src/luna.rs`,
`apps/prysm-webos-service/src/files.rs`,
[lifecycle notes](../developers/webos.md).

| Responsibility today | v1 |
| --- | --- |
| Luna `io.prysm.capture.service`: `status start stop setConfig cleanup` | `getState setPreferences setEnabled uninstall` (§6) |
| Only the app id may mutate, checked before parsing | Kept |
| Deferred replies until a helper finishes | Dropped: reply when accepted, effects by subscription |
| `tvpower` subscription, renewed after 5 s without a good reply, fail closed | Kept |
| Remote runs only when Active, embedded in Active/Screen Saver | One rule: awake = Active or Screen Saver; capture only in Active |
| Manual Stop survives standby; cold boot from `autostart` | Kept (intent in memory, seeded from `autostart`) |
| `videooutput` MAIN readiness, muted counts; context change resets | Kept ([observe.rs](../../hosts/webos-service/src/observe.rs)) |
| Embedded pause handshake (`p`, `CapturePaused`) | Dropped: target Off closes the source with certified release |
| Env config, one-byte stdin commands, inherited event fd with unsafe `pre_exec` | Typed argv, JSON lines on stdin/stdout, no `pre_exec` |
| `PR_SET_PDEATHSIG` + ppid recheck | Kept, in the worker at startup (safe `nix`), plus stdin EOF |
| stderr tail as `lastLog` | Last 20 lines in `State.log` |
| Encoder and 5 s video-reset watchdogs | Dropped (no H.264; native watchdogs live in [capture-webos](../../adapters/capture-webos/src/source.rs)); 15 s liveness timeout instead |
| TERM, KILL after 5 s; reap before restart | Kept; window 8 s |
| Backoff 1 s doubling, cap 5 s | 1 s doubling to 30 s, reset after 60 s healthy |
| `--verify-instance`, `Connected`/`ReceiverBusy` events | Dropped: a slim connect failure is a worker failure within 3 s |
| Capabilities, processing location, `embedded-prysm` feature | Dropped: two packages |
| Role manifests, rescan, activation, refuse unrelated files | Kept, behind `Installer` |
| Boot hook toggled with autostart, config rollback | Always installed by setup, removed by uninstall |
| `config*.json` migrations | Dropped |
| Probe/sandbox modes | Kept: `run --sandbox DIR --run-for S [--service-name N]` |
| Luna FFI safety contract; bindgen | Kept in `prysm-webos-luna-sys` |

## 3. Processes and crates

| Process (root) | Installed as (both IPKs) | Full IPK builds | Slim IPK builds | Owns |
| --- | --- | --- | --- | --- |
| supervisor (Luna-activated) | `prysm-webos-service` | `prysm-webos-service` | `prysm-webos-slim-service` | Luna name, power/video, `tv.json`, intent, worker lifecycle, setup/uninstall |
| worker (child) | `prysm-webos-worker` | `prysm-webos` | `prysm-webos-slim` | Full: engine, `WebosCapture`, WLED, portal. Slim: `Forwarder` into `RemoteSender` |

| Crate | Location | Contents | `unsafe` |
| --- | --- | --- | --- |
| `prysm-webos-link` | [crates/webos-link](../../crates/webos-link/src/lib.rs) | Protocol, codec, argv, exit codes; `worker` feature: stdio link, stop reasons, worker `main` | forbidden |
| `prysm-webos-luna-sys` | [adapters/webos-luna-sys](../../adapters/webos-luna-sys/src/lib.rs) | The only Luna/GLib binding: `run(Config, Handler)`, `check_registration` | own lints |
| `prysm-webos-service` | [hosts/webos-service](../../hosts/webos-service/src/lib.rs) | Machine, driver, launcher, preferences, installer, CLI; two thin bins | forbidden |
| `prysm-webos` | [hosts/webos](../../hosts/webos/src/main.rs) | Full worker, `EngineLink` surface | forbidden |
| `prysm-webos-slim` | [hosts/webos-slim](../../hosts/webos-slim/src/main.rs) | Slim worker, `ForwarderLink` surface, `Counted` sink | forbidden |

**One supervisor library, two thin binaries** (`src/bin/*.rs`, each one line
calling `prysm_webos_service::main(Flavor)`). The flavor is baked into the
binary, and each package ships only its own supervisor and worker under the
shared installed names, so a supervisor can never run the other package's
worker and no runtime flag can be set wrong. Names and paths are one set for
both flavors ([identity.rs](../../hosts/webos-service/src/identity.rs)). A single
binary with a `--flavor` in the `.service` file would save one link step and
add a way to mismatch. The supervisor stays separate from the worker so Luna
FFI and the engine never share a process and the worker cannot claim the name.

`capture-webos-sys` no longer carries its own four-function Luna table: its
`LSRegister` precheck calls `prysm_webos_luna_sys::check_registration`, so v1
has one Luna binding.

## 4. Supervisor ↔ worker contract

- **Start**: argv only, built without a shell
  ([args.rs](../../crates/webos-link/src/args.rs)):
  `prysm-webos --settings /var/lib/io.prysm.tv/settings.json --listen 0.0.0.0:8787 --lighting off`;
  `prysm-webos-slim --receiver 192.168.1.20:7380` (size, rate, menus and
  encoding come from the receiver's stream request, `prysm-3jx.63`).
- **Commands**: worker stdin. **Events**: the worker's original stdout. The
  worker duplicates fd 1 (CLOEXEC) for events and points fd 1 at stderr so
  vendor output cannot corrupt the stream. Logs: stderr.
- **Framing**: one JSON object per line, at most 4096 bytes with the newline,
  `deny_unknown_fields` (unit-like variants are `{}` structs so extra fields are
  rejected too). A bad, oversized or truncated line is a protocol failure: the
  supervisor terminates the worker; the worker exits `FAILED`.
- **Liveness**: status on every change and every 5 s; 15 s of silence is a hang.
- **Parent death**: `set_pdeathsig(SIGKILL)` then exit if `getppid() == 1`
  (Linux); stdin EOF also stops the worker. The supervisor spawns from its loop thread.

```rust,ignore
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Video { video: Video },              // unknown | unavailable | available; Unavailable unless power is Active
    ResetVideo {},                       // input, app or picture mode changed
    Lighting { target: LightingTarget }, // full only; a slim worker exits FAILED
}
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Status(WorkerStatus),                // every change and every HEARTBEAT (5 s)
    Faulted { cause: String },           // exit FAULTED follows
}
pub struct WorkerStatus {
    pub capturing: bool,                 // full: session Running and frames arriving; slim: frames in the last second
    pub fps: Option<u16>,                // frames/s over about a second while measured (full: session; slim: sending); 0 = waiting
    pub target: Option<LightingTarget>,  // full: the engine target, portal changes included
    pub portal: Option<String>,          // full: "http://192.168.1.5:8787/", looked up again at every heartbeat
    pub failure: Option<WorkerFailure>,  // { kind: encoder | receiver | other, message }; message cut to 1 KiB
    pub encoding: Option<SlimEncoding>,  // slim: the encoding sent while connected
    pub paused: bool,                    // slim: the receiver paused the stream; capture closed, not a failure
}
pub mod exit { STOPPED = 0 /* TERM, EOF */, FAILED = 1, USAGE = 2, FAULTED = 3 }
```

Worker side ([worker.rs](../../crates/webos-link/src/worker.rs)):
`worker::main(name, usage, parse, body)` parses argv (exit `USAGE`), guards the
parent, takes over stdio, installs SIGTERM/SIGINT, runs `body(args, Link)` on a
current-thread runtime and maps the outcome: a `Faulted` stop reason or
`Err(Error::Faulted)` → 3; a `Failed` stop reason or other `Err` → 1 after a
final `Status { failure }`; else 0.

- [`EngineLink`](../../hosts/webos/src/link.rs) (`Surface<EngineHandle>`):
  applies the initial target; `Video`/`ResetVideo` → `VideoSignal`, `Lighting`
  → `set_target`; snapshot changes and the heartbeat → `Status`; a session that
  stopped with a non-validation failure and a non-Off target is re-applied after
  1 s doubling to 30 s (reset after 60 s running); `SessionState::Faulted` →
  `Faulted` event and stop. The full host seeds `settings.json` with
  `capture: { kind: "webos" }` when missing, binds the portal listener itself
  (so it can report the LAN URL, looked up again at every heartbeat so a new
  TV address is advertised within 5 s) and uses a 6 s stop deadline.
- [`ForwarderLink`](../../hosts/webos-slim/src/link.rs) (`Surface<()>`):
  video commands into the capture; `capturing` sampled every second from the
  `Counted` sink. The slim host connects the `RemoteSender` before taking the device.

## 5. Supervisor state machine

[machine.rs](../../hosts/webos-service/src/machine.rs) is sans-IO:
`Machine::handle(Input, now) -> Vec<Effect>`; `Effect = Spawn(argv) | Send(Command)
| Terminate | Kill | Publish`. Inputs: `Power`, `Video`, `VideoContextChanged`,
`Preferences` (validated, persisted), `Enabled`, `Worker(Event)`, `WorkerLog`,
`WorkerBroken`, `SpawnFailed`, `WorkerExited(Exit)` (reaped), `Tick`, `Shutdown`.

`wanted = !shutdown && power ∈ {Active, ScreenSaver} && (full || (enabled && receiver set))`.
The video sent is `Unavailable` unless power is Active. Full: `enabled ⇔
lighting ≠ Off`; `setEnabled(true)` keeps a non-Off target, else Processed.

| Phase | Input | Next | Effects |
| --- | --- | --- | --- |
| Idle | `wanted` becomes true | Running | `Spawn`, `Send(Video)` |
| Running | effective video changes | Running | `Send(Video)` (also just before a stop) |
| Running | `VideoContextChanged` | Running | `Send(ResetVideo)` |
| Running | full `Enabled` | Running | `Send(Lighting)`; worker reports older than 2 s are ignored |
| Running | `Status` | Running | heard; full adopts `target` |
| Running | `wanted` false | Stopping (requested) | `Terminate` |
| Running | slim `Preferences` changing receiver or capture | Stopping (requested) | `Terminate` |
| Running | `Faulted`, `WorkerBroken`, 15 s silence | Stopping (failed) | `Terminate` |
| Running | `WorkerExited` | Retrying | failure = exit text + last status failure |
| Running | `SpawnFailed` | Retrying | — |
| Stopping | `Tick` ≥ 8 s after TERM | Stopping | `Kill` once |
| Stopping | `WorkerExited` | Idle (requested) or Retrying (failed) | then reconcile |
| Retrying | `Tick` at the retry time, or `wanted` false | Idle | then reconcile |

- Reap before restart is structural: `Spawn` only from Idle; only
  `WorkerExited` (after `waitpid`) leaves Running or Stopping.
- Backoff 1 s doubling to 30 s; 60 s running resets it. The supervisor's
  failure is cleared as soon as the worker is up again (full: a status without
  a failure; slim: frames flowing). Requested restarts have no backoff and no
  failure; a slim receiver or capture change restarts at once (also from
  backoff, or after a failed worker is reaped) with the delay reset.
- `capturing` and `fps` are published only while the worker is Running.
- `Retrying { seconds }` is the time left until the restart, rounded up to
  whole seconds and computed when the state is sent (a reply or a publish;
  time alone publishes nothing). The TV app counts down from when it received
  it, so no clock is shared and LS2's delivery delay is the only error.
- `Publish` whenever the published `State` changed; the driver also drops
  identical payloads.
- Persisted: `Preferences`, and the intent (`resume.json`: full target or slim
  enabled, with the kernel boot id). A service restart in the same boot (an
  update, a crash) resumes the intent; after a reboot it starts from
  `autostart`. The full target follows the worker and is passed as
  `--lighting` to the next worker.

| Seam | Production | Tests |
| --- | --- | --- |
| `Bus` (publish, resubscribe) | [luna.rs](../../hosts/webos-service/src/luna.rs) over `prysm-webos-luna-sys` | recording bus |
| `Launcher`/`Worker` | `ProcessLauncher`: pipes, reader threads, `kill(2)`, `try_wait`, reaper on drop | scripted fake; `sh` workers |
| `PreferencesStore` | `FileStore`: 0600, atomic replace, `O_NOFOLLOW`, 64 KiB | `MemoryStore` |
| `Installer` | `FsInstaller`: manifests, hook, bounded `ls-control` and `luna-send` | temp root with script stubs |

The driver ([service.rs](../../hosts/webos-service/src/service.rs)) runs on
the GLib thread: requests and subscription replies arrive as callbacks, a
100 ms timer drains worker output, reaps, ticks the machine and renews a
subscription without a good reply for 5 s. The sys crate defers bus actions
until the handler returns, so LS2 is never re-entered while it is borrowed.

## 6. Luna API

Both packages: service `io.prysm.tv.service`, app `io.prysm.tv`. Replies carry LS2
`returnValue`/`errorText`; payloads are objects of at most 4 KiB. The values
are exported as the `luna` OpenAPI document
([schema.rs](../../hosts/webos-service/src/schema.rs), generated
`@prysm/contracts/luna`): one document for both flavors, with flavor-specific
fields marked "Full only" or "Slim only".

| Method | Request | Reply | Caller |
| --- | --- | --- | --- |
| `getState` | `{ "subscribe"?: bool }` | `State` + `subscribed`, then every change | any |
| `setPreferences` | `Preferences` (complete) | `State` after it is persisted | app only |
| `setEnabled` | `{ "enabled": bool }` | `State` | app only |
| `uninstall` | `{}` | `{ "returnValue": true }`; then the worker stops, hook and manifests go, rescan, exit | app only |

```rust,ignore
pub struct Preferences {            // tv.json; unknown fields rejected
    pub autostart: bool,
    pub instance: Option<String>,   // slim: remote portal origin for the TV UI, http(s)://host[:port]
    pub receiver: Option<ReceiverAddress>, // slim: frame receiver, host:port (IP or name, resolved per connect); full rejects both
}                                   // picture settings: the portals' (prysm-3jx.63)
pub struct State {
    pub flavor: Flavor,             // full | slim
    pub preferences: Preferences,
    pub enabled: bool,
    pub lighting: Option<LightingTarget>, // full only
    pub power: Power,               // unknown | active | screen_saver | asleep
    pub video: Video,
    pub worker: Phase,              // { phase: stopped | running | stopping } or { phase: retrying, seconds }
    pub capturing: bool,
    pub fps: Option<u16>,           // the worker's frames/s while running and measured
    pub portal: Option<String>,
    pub failure: Option<WorkerFailure>, // { kind: encoder | receiver | other, message }
    pub encoding: Option<SlimEncoding>, // slim only
    pub paused: bool,               // slim only: the receiver paused the stream (lighting off)
    pub log: Vec<String>,           // last 20 worker stderr lines
}
```

The sender check uses LS2's application id, never the payload, before parsing.
Setup stays a Homebrew Channel `exec` of `setup.sh` (the service does not exist yet).

**Room for calibration.** Today's TV UI presents patterns itself (a `<video>`
in the app) and talks to the portal through the hidden
`calibration-tv.html` relay. v1 keeps that split: the full TV UI uses its own
portal (`State.portal`), the slim one `preferences.instance`. Neither the Luna
API nor the worker protocol is involved; anything calibration later needs from
capture is a portal setting (full: the TV's portal; slim: the desktop's
`remote` capture, sent to the TV as the stream request). Both enums
are tagged, so a TV-side step would be one added variant.

## 7. TV UI parity checklist

From `crates/web/prysm-webos/src/App.tsx`.
**Present** = the supervisor or an existing API serves it now.

| Current TV UI item | v1 | Status |
| --- | --- | --- |
| Status card: connecting, waiting for TV/video, capturing, disabled, needs attention | `State.power/video/enabled/worker/capturing/failure` | present |
| Primary Enable/Disable (remote), Pause/Allow TV capture (embedded) | `setEnabled` | present |
| "Set up service" when offline; Service options → set up or repair | `exec setup.sh` → `prysm-webos-service setup` | present (supervisor); needs packaging |
| Diagnostics: power, video, receiver connected, enabled, process running, error/last log | `State` | present |
| Remove service setup | `uninstall` | present |
| Processing location (remote/embedded) | two packages | dropped |
| Prysm instance URL | `setPreferences{instance, receiver}` | present; UI derives the receiver port from the remote `capture.listen` (remote portal API, exists) |
| Capture: video + menus / video only | slim `preferences.capture.include_ui`; full portal `capture.include_ui` | present |
| Compression (LZ4, H.264) | raw frames only | dropped |
| Enable with TV | `preferences.autostart` | present |
| Prometheus metrics (port 9101) | not in v1 | needs remote portal API (later) |
| Calibrate colors (QR pairing, pattern video, relay iframe) | portal calibration API + TV pattern player | needs calibration (`prysm-3jx.11`) |
| Portal address on the TV (embedded) | `State.portal` | present |
| Start/stop lights on the remote Prysm | remote `PUT target` | needs remote portal API (exists) reachable from `Origin: null` (Q1) |
| Receiver busy / verification before handover | — | dropped (v1 receiver replaces connections) |
| Drafts, focus, exit dialog, 30 s call timeout, reconnect on visibility | frontend | port in step 8 |

## 8. Settings on the TV

Root-owned 0700 state directory `/var/lib/io.prysm.tv` (both packages); one writer per file.

| File | Contents | Writer |
| --- | --- | --- |
| `/var/lib/io.prysm.tv/tv.json` | `Preferences` | supervisor |
| `/var/lib/io.prysm.tv/settings.json` | v1 `Settings` (`capture.kind = "webos"`, WLED, processing) | full worker's engine, via the portal; seeded by the worker |

Saved preferences that no longer validate are ignored (defaults, logged).
Uninstall keeps settings.

## 9. Packaging and install (not implemented here)

Both IPKs are the app `io.prysm.tv` with identical installed file names;
installing one replaces the other.

| IPK | App | Service directory `…/services/io.prysm.tv.service/` |
| --- | --- | --- |
| full | `dist/webos/tv-ui` | `prysm-webos-service` (from bin `prysm-webos-service`), `prysm-webos-worker` (from `prysm-webos`), `setup.sh`, role/service/permission files, licences |
| slim | `dist/webos-slim/tv-ui` | `prysm-webos-service` (from bin `prysm-webos-slim-service`), `prysm-webos-worker` (from `prysm-webos-slim`), same file names |

- Manifests written by `setup`: `<app>.supervisor.json` (roleFiles
  `supervisor.role.json`, serviceFiles `supervisor.service`, client and api
  permission files) and `<app>.worker.json` (roleFiles `worker.role.json`).
- Worker role: `exeName` the installed worker, `allowedNames`
  `com.webos.service.capture.client*` and `com.webos.rm.client.*`, outbound as
  `packaging/webos/service/capture.role.json`.
  Supervisor role: its service name; outbound `com.webos.service.tvpower`,
  `com.webos.service.videooutput`. `supervisor.service` execs the supervisor without arguments.
- `setup.sh`: `exec "$(dirname "$0")/prysm-webos-service" setup`. Setup writes both
  manifests and `/var/lib/webosbrew/init.d/io.prysm.tv` (refusing unrelated
  content; re-running it on the same install succeeds), rescans (10 s bound) and activates with `luna-send … getState`
  (10 s bound, `returnValue` checked). The boot hook runs the same `setup`.
- To change: `packaging/webos/build-ipk.sh` builds
  `-p prysm-webos -p prysm-webos-service` or the slim pair from `v1/Cargo.toml`
  after the v1 frontend, one IPK each, renaming the binaries to the shared
  installed names above, and drops `--hardware-h264`,
  `--embedded-prysm`, the generated embedded role and GStreamer/OpenH264
  licences; `secure-ipk.py` and `packaging/webos/tests` get the two payload
  allowlists; `xtask/src/main.rs` `package webos` gets a
  `--slim` choice and drops the TV feature flags; `codegen` drops
  `Contract::Webos` and moves the bindgen step to `webos-luna-sys`;
  `packaging/webos/tests/native-smoke.py` uses the new
  names and `run --sandbox`. Docs: rewrite [webOS lifecycle](../developers/webos.md)
  and [TV setup](../users/webos.md) on ship.

## 10. What needs what

| Needs | Covered |
| --- | --- |
| Dev host (done) | Codec and limits; argv; machine tables with explicit time; driver with fakes; `ProcessLauncher` against `sh` workers (TERM→KILL, reap, garbage, oversize, drop); driver + real launcher + script worker; both worker surfaces against runtime fakes; worker binaries' exit codes (usage, EOF, garbage, unreachable receiver, off-TV capture); preferences and installer in temp roots; Luna wrapper on recording stand-in tables; supervisor binaries refuse to run off the TV |
| webOS SDK | bindgen `--check` for `webos-luna-sys`; armv7 build and link of all four binaries and the `capture-webos-sys` C++; Miri on the stand-in tests; IPK build and payload tests |
| Rooted TV | Registration and app-id check, power and video subscriptions, `pdeathsig`, real capture and release, standby/wake, cold boot via hook, hung-capture KILL, slim into headless, portal on the LAN from the TV app origin, uninstall |

TV tests keep the current rules: probe identity (`run --sandbox` under a
private `/tmp/prysm-supervisor.*`), bounded `--run-for`, exclusive probe
manifest, `finally` cleanup, never an unbounded production service as a probe.

## 11. Implementation sequence

| # | Step | Verify | Status |
| --- | --- | --- | --- |
| 1 | `webos-link`: types, codec, argv, exit codes, worker side | unit tests | done |
| 2 | `hosts/webos` and `hosts/webos-slim`: mains, link surfaces, `Counted`, exit mapping | surface tests on runtime fakes; process tests | done |
| 3 | Supervisor machine, preferences, power/video parsing | table tests | done |
| 4 | `ProcessLauncher` and driver over a fake `Bus` | `sh` worker tests | done |
| 5 | `webos-luna-sys` (port, one binding for both sys crates) and the real `Bus` | stand-in tables; SDK `--check` | done on dev host; SDK check pending |
| 6 | `Installer`, `setup`/`uninstall`/`run --sandbox` CLI | temp-root tests | done |
| 7 | Packaging, xtask, bindgen tool, smoke script (§9) | IPK on SDK; TV smoke | open |
| 8 | TV UI bundles on the new Luna API and the portal API (§7) | frontend tests; TV acceptance | open |
| 9 | Calibration (`prysm-3jx.11`) | portal + TV | open |
| 10 | TV acceptance, then delete `apps/prysm-webos-*`, `crates/web/prysm-webos`, `prysm-webos-protocol` | TV | open |

## 12. Open questions (with defaults)

1. **Portal access from the TV app.** The TV app is a `file://` page
   (`Origin: null`); `Access::Origins` rejects `null`, so the full worker uses
   `Access::AnyOrigin` and a slim TV needs the headless portal on `AnyOrigin`.
   Default: as is. Recommended: an `Access` option for exactly `null`, or route
   all remote calls through a portal-served relay page (as calibration does)
   and keep `SameOrigin`.
2. **Slim `instance` and `receiver`.** Both stored; the TV UI fills `receiver`
   from the remote `capture.listen` port. Default: keep both.
3. **Unconfirmed release in slim.** `Forwarder` returns it as a plain error, so
   the worker exits `FAILED`, not `FAULTED`; both restart the process. Default: accept.
4. **A worker that survives SIGKILL** (uninterruptible kernel wait) keeps the
   supervisor in Stopping; no second worker starts. Default: accept (TV reboot).
5. **Role `inbound` narrowing** from `*` to the app, and whether
   `/var/luna-service2/manifests.d` survives reboot. Default: keep the runtime
   sender check and the boot hook until a TV test says otherwise.
