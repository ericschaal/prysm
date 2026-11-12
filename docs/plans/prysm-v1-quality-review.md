# v1 code quality review (bead prysm-3jx.66, review half)

Reviewed at HEAD 60f6bae (`codex/v1-architecture-skeleton`), read-only. Tools run: `cargo clippy --workspace --offline` (lib/bin targets only, with
`unwrap_used, expect_used, panic, indexing_slicing, unreachable, let_underscore_must_use, pedantic, unreachable_pub`) to list risky sites, and the
repository's root `eslint.config.mjs` against `v1/frontend` with `--no-ignore` (v1 is ignored by default; see F1).

## Summary

| Area | Verdict | One line |
| --- | --- | --- |
| 1. ports / runtime / protocol | **fine** | The engine/session/worker split is clear and small. Main costs: helpers copied into other crates (chain, target/session labels), a public surface wider than needed, one 55-line closure in `Session::start`, and protocol copies of ports enums. |
| 2. Adapters (incl. 3 sys crates) | **needs work** (architecture), sys crates **good** | Unsafe code is careful: every block has a SAFETY note, handles are `!Send`, panics in callbacks abort. But three adapters each run their own owner thread with their own mutex, condvar and release deadline, and capture-device runs a tokio/futures bridge thread. The README's "adapters are plain objects" rule is not met there. |
| 3. Hosts | **fine** | Desktop `main`s are just builder lists, and the supervisor is a pure, table-tested state machine. Both are the patterns to keep. Problems: retry/backoff is implemented twice, the TV worker status allows illegal states, encoder failures are found by downcasting, a `#[path]` include shares a file across crates, webos-service I/O returns `String` errors. |
| 4. Surfaces / tools | **fine** | Small, readable. More copies of chain and the labels; small dead API. |
| 5. Frontend | **needs work** | No lint gate at all (117 problems under the root config). Helpers are duplicated between tv-ui and web-portal, enum lists are hand-written, the UI branches on error-message text and on a `field: 'target'` string, and two components are 270–400 lines long. |
| 6. Packaging / CI | **fine** | Scripts are strict (`set -eu`, bash with `pipefail`). Two gaps: the arm-only unsafe code is never clippy-checked, and frontend lint (F1). |

Counts: **must 1, should 29, could 27** (by area: 1: 0/9/8, 2: 1/6/9, 3: 0/6/4, 4: 0/0/4, 5: 0/7/1, 6: 0/1/1).
Clippy found no `unwrap()`, `todo!` or unguarded `panic!` in production code. The few production `expect`/`panic`/`unreachable` calls are on invariants
(`AdapterSettings::new` ports/settings.rs:83, `logging.rs:52`, `hosts/common/args.rs:131`, `webos-service/identity.rs:111`,
`webos-slim/check.rs:120`, `tui/lib.rs:102`, `webos-link/args.rs:228`). The 51 `indexing/slicing may panic` sites are all bounds-checked first, in wire, NAL and HTTP parsers. 50 `let _ =` sites
drop `#[must_use]` results; all are reply/notify sends or best-effort cleanup, which is acceptable.

### Top changes by value/effort

1. **Lint the v1 frontend in CI** and fix what that finds (F1). Today nothing lints `v1/frontend`. S–M.
2. **One home for shared helpers**: `prysm_ports::chain` everywhere (6 copies plus a re-export), and `LightingTarget::{ALL, as_str}` / `Display for SessionState`
   in ports (5 copies with three different spellings). Then `failureOf` / `equal` / `describeLighting` once in the frontend (P1, P2, S1, F2). S.
3. **Type the calibration conflict** instead of `ValidationError::new("target", …)` and the frontend's `reason.field === 'target'`. Use `failure.kind` rather than message regexes (P10, F4). S.
4. **Rename the settings store** `prysm_settings_file::FileSettings`, which clashes with `prysm_capture_file::FileSettings` (A7). S.
5. **Narrow the runtime/ports API**: make `Session`, `SessionEnd`, `OpenOutput`, `RELEASE`, `PreviewSlot` and `Stats` `pub(crate)`. Drop
   `#[non_exhaustive]` on workspace-internal enums. Merge `session` and `meter` into one `Option<Running>` (P4, P5, P6). S.
6. **Make TV worker status illegal states unrepresentable**: `failure: Option<WorkerFailure>` instead of the paired `failure`/`failure_kind`, a slim
   uplink state enum instead of `paused`/`waiting`/`encoder` with precedence rules, and a typed encoder failure instead of downcasting to `H264Error` (H4, H5). M.
7. **Remove duplicate infrastructure in hosts**: one resolver-with-deadline (A3), one LAN-address probe (H3), no `#[path]` include (H2), one retry policy (H1). S–M.
8. **Split `Session::start`'s loop closure** into a `SessionLoop` struct with `on_frame` / `on_quiet` / `finish` (P8). M.
9. **Frontend wizard and TV app**: `useReducer` for `CalibrateWizard`'s 9 `useState`s, split its screens, and fix the effect dependencies in `TvApp` (F6). Add a decode round-trip test for `qr.ts` (F7). M.
10. **One owner-thread helper** for push-style native sources (A2). At root consolidation, give `prysm-capture` a blocking API so capture-device drops tokio, tokio-util and futures (A1). L.

---

## 1. `crates/ports`, `crates/runtime`, `crates/protocol`, `crates/webos-link`

Good, keep: `LatestSlot` (ports/slot.rs), the `Leds` enum in session.rs:146-201 (an explicit hold-state machine for the output),
`Worker`/`Flags` shared by Session and Forwarder (worker.rs), `Stats` uses only relaxed atomics on the hot path, the `Surface` blanket impl for
closures, the `Registry` type erasure (registry.rs:113-148).

- **P1 should S** — The display names of `LightingTarget` and `SessionState` are re-implemented in five places, with inconsistent words:
  `hosts/common/src/lib.rs:365-380` ("device native"), `surfaces/tui/src/command.rs:59` ("native"), `surfaces/tui/src/view.rs:256`,
  `surfaces/web-portal/src/metrics.rs:16` ("device_native"), and `crates/webos-link/src/args.rs:224-230`, which goes through `serde_json::to_value`
  plus `unreachable!`. Change: add `LightingTarget::ALL`, `LightingTarget::as_str()` (the serde name) and `impl Display for SessionState` in
  ports/snapshot.rs. Use them everywhere, and keep human wording only where a UI needs it.
- **P2 should S** — `prysm_ports::chain` exists, but copies remain: `adapters/capture-webos/src/source.rs:349` (production),
  `surfaces/tui/src/command.rs:68` (`describe`), test copies in `capture-device/src/tests.rs:98`, `capture-file/src/tests.rs:66` and
  `capture-webos/src/tests.rs:52`, and `capture-remote/examples/receive.rs:99`. `prysm-webos-link` also re-exports it twice (`lib.rs:132`,
  `worker.rs:8`), and hosts import it as `prysm_webos_link::worker::chain` (`hosts/webos/src/link.rs:6`). Change: delete the copies
  and the re-exports, and import `prysm_ports::chain`.
- **P3 should M** — protocol mirrors ports enums 1:1: `Input` (lib.rs:79-98), `OutputIssue` (lib.rs:100-129), `Phase` (lib.rs:159-179). Yet
  ports already serializes `LightingTarget`, `IdleMode` and `Calibration`. Change: derive `Serialize`/`Deserialize` for `OutputIssue` and `Input` in ports
  (snake_case) and delete the protocol copies and their `From` impls. Keep `Session` (it is tagged differently). Also fix the name drift
  `SessionPhase::Finished` ↔ wire `Phase::Finishing` (lib.rs:167,176): pick one word.
- **P4 should S** — `#[non_exhaustive]` on `prysm_ports::Error` (error.rs:23) only matters to external crates, and there are none. It forces
  `_ => FailureKind::Internal` in `protocol/src/lib.rs:248`, so a new variant (as `Panicked` already does) silently becomes `internal`. Same
  pattern on `capture-webos-sys/src/vtcapture.rs:37`, `capture-remote/src/wire.rs:77`, `h264/src/lib.rs:49`, `h264-rvenc-sys/src/lib.rs:30`. Change:
  remove the attribute and match exhaustively.
- **P5 should S** — runtime exports internals only it uses: `Session`, `SessionEnd`, `OpenOutput`, `RELEASE`, `PreviewSlot` and `Stats`
  (lib.rs:29-32; grep shows no use outside `crates/runtime/src`). `Registry::source_kinds`/`output_kinds` (registry.rs:60-66) have no callers.
  Change: make them `pub(crate)` and delete the unused ones. Keep `Counters`/`Timing` (used by metrics) and `Preview`/`PreviewWatch`.
- **P6 should S** — `Core.session: Option<Session>` and `Core.meter: Option<Meter>`, documented as "present exactly while" (engine.rs:265-267), are
  an invariant enforced by hand in `start`, `stop` and `session_ended`. Change: `running: Option<Running { session, meter }>`. `measure()` and
  `finished()` then take one option.
- **P7 could S** — `Core::run` repeats `match self.faulted() { Some(fault) => Err(fault), None => … }` three times (engine.rs:307-330). Check once
  for the three command arms, for example `fn guarded<T>(&mut self, f) -> Result<T>`.
- **P8 should M** — `Session::start` (session.rs:232-308) is mostly a 55-line closure with four captured mutable locals (`processor`, `leds`,
  `last`, `last_frame`) and two paths (frame, quiet tick) mixed through early returns. Change: a private `struct SessionLoop { processor, leds,
  last, last_frame, open_output, shared, preview, stats }` with `on_frame(frame)`, `on_quiet()` and `finish(mode) -> SessionEnd`, so the closure is
  three lines and each rule can be read on its own.
- **P9 could S** — `Leds::write` (session.rs:157-166) checks `matches!(Held)` and then matches again with an unreachable arm. Use
  `let output = match self { Held(o) => o, _ => { *self = Held(open()?); … } }`.
- **P10 should S** — Calibrating while not lighting returns `ValidationError::new("target", …)` (engine.rs:533-537), but `field` is documented as a
  settings dotted path (error.rs:74, protocol lib.rs:189), and "target" is not a setting. The frontend relies on that string
  (`frontend/packages/web-portal/calibrate.tsx:197`). Change: a dedicated `Error::NotLighting` (or `Conflict`) mapped to a new `FailureKind`.
  Have the frontend switch on `kind`.
- **P11 should M** — Adapters hard-code their section into field paths: `"capture.fps"` (`capture-remote/src/request.rs:125`,
  `capture-webos/src/settings.rs:64`), `"output.address"` (`output-wled/src/settings.rs:77`), and
  `format!("{}.encoding", Section::Capture.as_str())` (`capture-remote/src/receiver.rs:259`). The `Registry` already knows the section
  (registry.rs:76-79). Change: factories return paths relative to their section (`"fps"`), and `Registry::validate`/`open` prefix them. The
  path `"correction"` in `profiles.rs:126` is not under `processing.profiles…` as the other profile paths are; make it consistent. (`Failure.field`
  itself stays a string: it is wire data.)
- **P12 could S** — `sample_rgb` returns `Result<_, &'static str>` (sample.rs:12-17). Use a small error enum (`Empty`, `Compressed`).
- **P13 could S** — Stale and duplicated: the comment at registry.rs:9 ("when the OpenAPI export moves here") describes a plan that went
  elsewhere (protocol/schema.rs), and `#[schema(max_items = 32)]` (protocol/src/schema.rs:76) hard-codes `MAX_PROFILES`.
- **P14 could S** — `FrameSink::idle(&mut self)` (adapter.rs:123) shares a name with `LedOutput::idle(self, IdleMode)` but means "no frame this
  tick". Rename it `FrameSink::tick` (or `poll`) and update `RemoteSender` and the slim `Uplink`.
- **P15 could S** — `Snapshot::failure` says it is "cleared when lighting next starts successfully" (snapshot.rs:67, protocol lib.rs:52), but
  `Core::record` clears it on any successful command, including `Off` (engine.rs:561-564). Fix the doc, or the code.
- **P16 could M** — `protocol/src/schema.rs` keeps about 250 lines of schema-only mirrors so that ports stays free of `utoipa` (CONTRACTS.md).
  An optional `utoipa` dependency in ports with `cfg_attr(feature = "schema", derive(ToSchema))` would delete the mirrors and their shape tests.
  This reverses a recorded decision, so it is the owner's call.
- **P17 could S** — If the session thread cannot be spawned, the source is dropped without `close` (session.rs:233-234, documented). Since
  `close` certifies release, return the source to the caller on spawn failure and close it there.
- **webos-link (part of P2/H4)** — `WorkerStatus` mixes full-only fields (`target`, `portal`) and slim-only fields (`encoding`, `paused`) in one
  struct (lib.rs:65-90); see H4.

## 2. Adapters

Good, keep: all three `-sys` crates set `unsafe_op_in_unsafe_fn = deny` and `clippy::undocumented_unsafe_blocks = deny`. Every `unsafe` block I
read has an accurate SAFETY note. `VtCapture` and `UiCapture` are `!Send` (raw pointers) as LG requires. `VtCapture::open` leaks the library on
a failed create (vtcapture.rs:247-249), on purpose. Luna callbacks are wrapped in `guarded` (`catch_unwind` → abort; service.rs:661-676). A failed
`LSUnregister` aborts instead of freeing state LS2 may still use (service.rs:578-586). `driver_plane` ties the slice lifetime to the owner
borrow (vtcapture.rs:157). Also good: the WLED adapter is a plain object with a non-blocking observer (observe.rs), and the capture-file
subprocess handling is bounded.

- **A1 must L (architecture)** — `prysm-capture-device` violates "Adapters are plain objects… no channels". It spawns a bridge thread that drives the
  root crate's async API with `futures::executor::block_on` over tokio `oneshot`/`watch` and a tokio-util `CancellationToken`
  (`adapters/capture-device/src/source.rs:1-16,127-149`; Cargo deps `futures`, `tokio`, `tokio-util`). It then re-implements a frame slot plus
  exit state with `Mutex<State>` and a `Condvar` (source.rs:99-125). The cause is upstream: `prysm_capture::DeviceCapture` is async. Change, at root
  consolidation: give `prysm-capture` a blocking `open/next_frame/close` API, and the adapter becomes a thin wrapper without those three deps.
- **A2 should M** — Three adapters each hand-roll "a native owner thread, shared state (mutex + condvar or atomics), newest frame, terminal
  failure, certified release with a deadline": capture-device `Shared` (source.rs:99-125, 151-257), capture-webos `Shared` plus its native thread
  (source.rs:97-130, 380-420), and capture-remote `RemoteSource` (receiver.rs:156-238: `LatestSlot`, two `AtomicBool`s, `Mutex<Option<Refusal>>`,
  its own thread). Change: one helper next to `LatestSlot` (for example `prysm_ports::OwnerThread<Msg>` with spawn, publish frame, fail, release
  within a deadline, and a watchdog hook). Each adapter keeps only its native calls. That is what "concurrency lives in one place" asks for.
- **A3 should S** — Two resolve-with-deadline helpers with different semantics. `output-wled/src/address.rs:212-235` spawns a new thread for
  every lookup and abandons it on timeout, so a hanging resolver accumulates threads, one per reopen. `hosts/webos-slim/src/resolve.rs:40-104`
  keeps at most one pending lookup. Change: one `Resolver` (the slim semantics), used by both.
- **A4 should S** — The remote refusal "scope" is a raw `u8` flag on the wire API (`wire.rs:207-214` `refusal(reason, flags: u8)`, `sender.rs:122`
  `refuse_with(reason, flags: u8)`) and a `bool` on the receiver (`receiver.rs:244-248` `encoding: bool`). Change: `enum Refused { Request,
  Encoding }`, encoded to and decoded from the bit only in `wire.rs`.
- **A5 should S** — `RemoteSource::set_wanted` (receiver.rs:212-219) and the `Wake.wanted/told` machinery (receiver.rs:299-325) are driven only by
  tests and examples (`tests/h264.rs:357`, `tests/loopback.rs:435`, `examples/receive.rs:55`). In production, pause happens through `close`. Remove
  it, or wire it to the engine if pausing without closing is wanted.
- **A6 could S** — `capture-remote` re-exports constants nobody outside uses (`POLL_INTERVAL`, `CONNECT_TIMEOUT`, `WRITE_TIMEOUT`,
  `MAX_DIMENSION`, `MAX_PAYLOAD_BYTES`, `MAX_REASON_BYTES`, `VERSION`; lib.rs:17-20), and `RemoteSender::peer` has no caller. Make them `pub(crate)`.
- **A7 should S** — Name clash: `prysm_settings_file::FileSettings` (a `SettingsStore`, settings-file/src/lib.rs:11) and
  `prysm_capture_file::FileSettings` (the `"file"` source's settings). Every host imports the former next to `FileSource`. Rename the store
  `SettingsFile` (or `JsonSettingsStore`).
- **A8 could S** — The WLED observer turns every failure into `Unreachable` and drops the cause (observe.rs:70,78). A reply that is not the
  expected JSON is also "does not answer" (observe.rs:72). Log the cause once at debug level, and consider a separate "unexpected reply" state.
- **A9 could S** — `VideoSignal(Arc<Mutex<(Video, u64)>>)` (capture-webos/src/video.rs:20) should be a named struct `{ video, generation }`.
- **A10 could M** — `capture-webos/src/source.rs` (494 lines, banner comments `// ---- Native seam ----`) holds the seam traits, timing limits,
  shared state, the native thread loop and the `FrameSource` impl. Split the thread loop into `native.rs`. Mostly resolved by A2.
- **A11 could S (sys crates)**
  - The comments "One of the two v1 crates allowed to use `unsafe`" are stale: there are three (`capture-webos-sys/Cargo.toml:22`,
    `webos-luna-sys/Cargo.toml:15`).
  - The `api!` macro wraps the whole struct construction, including `?`, in one `unsafe` block (vtcapture.rs:81-87), as does halgal.rs:85-99.
    Narrow the block to the `library.get` calls.
  - `halgal::hal()` (halgal.rs:72-104) says the library is "never unloaded: unloading leaks file descriptors", yet on a missing symbol the
    `?` drops (unloads) it. `forget` it on that path too, or say it does not matter there.
- **A12 could S** — `webos-luna-sys`: `Bus::recall(index: usize)` indexes `Config::calls` (service.rs:57, 383-386 silently skips an out-of-range
  index); a typed `CallId` would be better. A reply that arrives while the handler is borrowed is dropped without a log line (service.rs:425-427).
- **A13 should S (known flaky)** — `capture-device` `real_backend_missing_device_fails_without_hanging` (tests.rs:388) bounds `DeviceSource.open`
  by `LIMIT = 5 s` (tests.rs:14), but the adapter waits up to `Deadlines::DEVICE.ready = 10 s` (source.rs:66-70) when the native backend
  does not fail at once. Likely cause of the flake (not reproduced). Change: assert within `ready + release`, or build the source with short
  `Deadlines` through the existing seam. Do not race a 5 s limit against a 10 s deadline.
- **A14 could S** — Wall-clock performance assertions that can fail on a loaded CI runner: `output-wled/src/tests.rs:733-734` (`frames > 50`,
  `slowest < 100ms`), `crates/runtime/tests/engine.rs:735` (`fps > 20.0`). Loosen them or move them to a benchmark.

## 3. Hosts

Good, keep: `hosts/headless/src/main.rs` and `hosts/tui/src/main.rs` are only a builder list. `prysm_host::main` owns the process concerns.
`webos-service/src/machine.rs` is a pure state machine (inputs and time in, effects out) with table tests. `worker::main` gives both TV
workers one process skeleton.

- **H1 should M** — Retry/backoff exists twice with identical constants: the full worker re-applies a failed session
  (`hosts/webos/src/link.rs:11-15,148-202`, `RETRY_MIN/MAX/HEALTHY`), and the supervisor restarts the worker
  (`webos-service/src/machine.rs:17-21`, same names and values). Desktop hosts do not retry at all. Session retry is runtime policy living in a host
  surface. Change: either `Builder::retry(policy)` in the engine (then every host behaves the same and `EngineLink` only relays), or drop the
  in-worker retry and let the supervisor own it. Owner's choice.
- **H2 should S** — `#[path = "../../webos/src/video.rs"] mod video;` (`hosts/webos-slim/src/main.rs:7-8`) shares one source file between two
  crates. Change: define `Video` once, in `prysm-webos-link` (protocol crate, no heavy deps). Have `prysm-capture-webos` use it, which also
  removes the 1:1 copy `capture-webos/src/video.rs:5-15` and the mapping in `hosts/webos/src/video.rs:12-21`. Tests can use `VideoSignal` directly,
  and the `VideoFeed` trait goes away.
- **H3 should S** — The LAN-address probe (a connected UDP socket to 192.0.2.1) is duplicated: `hosts/common/src/lib.rs:239-249` and
  `hosts/webos/src/main.rs:85-94`. Change: one `prysm_web_portal::lan_url(SocketAddr) -> Option<String>`.
- **H4 should M** — Paired fields with an "exactly when" rule. `WorkerStatus { failure, failure_kind }` is "set exactly when"
  (`crates/webos-link/src/lib.rs:76-80`), and the same holds for `webos-service/src/state.rs:77-83`. The slim `Report { requested, encoding, paused,
  waiting, encoder }` (`hosts/webos-slim/src/link.rs:32-47`) is decoded into a status by precedence rules in `Flow::status` (link.rs:67-83). Change:
  `failure: Option<WorkerFailure { kind, message }>` on the wire and in the Luna state (no compatibility constraint), and a slim uplink enum
  (`Waiting(String) | Sending { encoding } | Paused | EncoderFailed(String)`) plus `requested`.
- **H5 should M** — The slim worker classifies an encoder failure by walking the cause chain and downcasting to `H264Error`
  (`hosts/webos-slim/src/link.rs:261-280`). But `RemoteSender::send` already knows it is an encoder failure: it refuses at `sender.rs:233-239`.
  Change: return a typed outcome from the sender (for example `Error` with a distinguishable source type exported by capture-remote, or
  `SendError::Encoder`) and match on it.
- **H6 should S** — webos-service runtime I/O uses `Result<_, String>`, so causes are lost: `PreferencesStore` (preferences.rs:80-95; 9 sites), `install.rs`
  (10 sites), `luna.rs`, `service.rs`. Change: `thiserror` enums with `#[source]`. (`String` errors in argument parsing — `webos-link/args.rs`,
  `hosts/common/args.rs`, `webos-service/cli.rs` — are usage text and fine.)
- **H7 could S** — The `Status` surface prints status and also performs `--start` (`hosts/common/src/lib.rs:277-301`). Split out a `start`
  surface, or add a builder option for the initial target (the TV worker has the same need: `EngineLink.initial`).
- **H8 could S** — The desktop adapter list appears three times (`hosts/headless/src/main.rs:28-32`, `hosts/tui/src/main.rs:37-41`,
  `tools/export-contracts/src/main.rs:33-40`). A small `Register` trait implemented by `Builder` and `Contract` would let one function list
  them.
- **H9 could S** — `self.retry[Watch::Power as usize]` array indexing by enum discriminant (`webos-service/src/service.rs:272,285,314`). Use a struct with
  one field per watch.
- **H10 could S** — `Machine { enabled, lighting }`: "Slim: enabled; Full: derived from lighting" (machine.rs:103-107). Use one flavour-shaped intent
  enum.

## 4. Surfaces and tools

- **S1 (covered by P1/P2)** — `surfaces/tui/src/command.rs:59,68` and `view.rs:256` duplicate the labels and `chain`; `web-portal/src/metrics.rs:16`
  duplicates the target names.
- **S2 could S** — `surfaces/tui/src/lib.rs:102` polls `async { running.as_mut().expect("guarded").await }` under a select guard. Use
  `futures::future::OptionFuture`/`Fuse`. The TUI mixes std and tokio mpsc around its screen thread (lib.rs:47-48, screen.rs). Acceptable,
  because the terminal is blocking I/O, but `spawn_blocking` plus one channel type would be simpler.
- **S3 could S** — `web-portal/src/metrics.rs` (the hand-rendered `/metrics`) is fine: closed label sets, no escaping needed. Seven `let _ =
  writeln!` lines (metrics.rs:80-156) could become one `fn line(&mut self, …)` helper.
- **S4 could S** — `WebPortal::new(listen, …)` and `Listen::Address` (web-portal/src/lib.rs:47-62) are used only in tests; every host uses
  `from_listener`.
- **S5 could S** — `tools/export-contracts/src/fixtures.rs:123` is a 134-line function; split it per document.

## 5. Frontend (`v1/frontend`)

Good, keep: host assemblies (`hosts/*/web-portal/app.tsx`) type `AdapterUis<…>` exhaustively over the generated unions. STATE.md's "engine is
authoritative, store holds the latest state, drafts stay out of Redux" holds in `application-state`.

- **F1 should S–M (gate)** — `v1/frontend` has no ESLint config, and the root `eslint.config.mjs:21` ignores `v1/**`, so v1 TypeScript is not
  linted anywhere (v1.yml runs only codegen, typecheck and tests). With the root config forced on (`--no-ignore`): **117 problems (96 errors, 21
  warnings)**:
  - 71 × `react-refresh/only-export-components`
  - 17 × `no-nested-ternary`
  - 3 × `@typescript-eslint/unbound-method` (`tv-ui/index.tsx:190-191`, `web-portal/preview.tsx:127`)
  - 3 × `no-unnecessary-type-assertion` (`tv-ui/index.tsx:193`, `tv-ui/portal.tsx:79`, `web-portal/section-form.tsx:74`)
  - 2 × unused vars (`web-portal/processing.tsx:73-74`)
  - 1 × `no-control-regex` (`web-portal/fields.tsx:215`)
  - 2 × "rule `react-hooks/exhaustive-deps` not found" (`tv-ui/index.tsx:115,154`: disable comments for a plugin that is not configured)
  - 1 unused disable directive (`tv-ui/index.tsx:187`)
  - 1 × `require-await` (`testkit/preview.ts:20`)
  - warnings: 5 × `set-state-in-effect`, 3 × `exhaustive-deps` (`tv-ui/index.tsx:116,155`, `web-portal/processing.tsx:165`), 4 ref names,
    4 × `useContext`→`use`, 1 array-index key (`web-portal/index.tsx:290`)

  Change: add a v1 eslint config (or extend the root one to v1 with its own install), add `npm run lint` to v1.yml, and fix the findings. Most are
  mechanical: move non-component exports out of `.tsx`, replace nested ternaries.
- **F2 should S** — Duplicated between tv-ui and web-portal:
  - `failureOf` (`web-portal/adapters.ts:89`, `tv-ui/portal.tsx:54`), identical.
  - Deep equality `same` (`web-portal/adapters.ts:73`) and `equal` (`tv-ui/model.ts:21`). These differ: `same` uses `Object.is` and the
    other `===`.
  - `describeLighting` (`web-portal/lighting.tsx:33`, `tv-ui/portal.tsx:131`) with diverging wording ("Screen sync is running." vs "…is on.",
    and the faulted text).

  Change: put them once in `application-state` (or a small `@prysm/application-text`) and import them in both.
- **F3 should S** — Wire enums are re-listed by hand in runtime validators and can drift from the generated types:
  `application-client/http.ts:207-208` (targets, sessions) and `webos-platform/index.ts:101-105` (flavors, powers, videos, phases, targets).
  Change: `as const satisfies readonly LightingTarget[]` plus an exhaustiveness check, or emit enum value arrays from codegen.
- **F4 should S** — The UI branches on strings: `tv-ui/portal.tsx:90` matches the error message against `/refuses requests from this page|origin
  denied/` instead of `failure.kind === 'forbidden'`, and `web-portal/calibrate.tsx:197` checks `reason.field === 'target'` (see P10). Change:
  branch on `kind`.
- **F5 should S** — `usePortal().setTarget` uses `runtime!` twice (`tv-ui/portal.tsx:117-118`) and throws a `TypeError` when there is no runtime
  (unsupported or no base URL). `endCalibration` right below guards against this. Guard the same way.
- **F6 should M** — `CalibrateWizard` (`web-portal/calibrate.tsx:148-547`) is one ~400-line component with 9 `useState`s whose transitions are
  coupled (`draftId`, `previous`, `screen`, `busy`, `problem`, `outcome`, `seen`, plus a `setState` during render at line 172). Move it to a
  `useReducer` with named actions and split the start, step and outcome screens. `TvApp` (`tv-ui/index.tsx:72-342`, about 270 lines) has the
  effect-dependency problems listed in F1. Extract the calibration and visibility effects into hooks.
- **F7 should S** — `tv-ui/qr.ts` (a hand-written QR encoder) is checked only against hashes it produced itself (`tests/tv.test.jsx:1044-1052`), so
  a wrong format-bit or mask placement would never fail a test. Change: a round-trip test decoding versions 1–6 with a dev-only decoder (for
  example jsQR), or replace it with a small vetted library.
- **F8 could M** — tv-ui `usePortal` rebuilds a React binding (`useSyncExternalStore` over `runtime.store`, `tv-ui/portal.tsx:62-80`) alongside
  `application-react`. Reuse the provider and hooks with a runtime per base URL.
- `tv-ui/patterns.ts` (the MSE pattern stream) is fine: bounded buffer, cleanup in one `stop`, stall detection. Nothing to change.

## 6. Packaging / CI (brief)

- **C1 should S** — The arm-only unsafe code (`h264-rvenc-sys/src/gst.rs`, compiled only for `linux/arm`) is never clippy-checked. The armv7 job
  runs `cargo check` (v1.yml:206-208) and is `continue-on-error: true` (v1.yml:173), so `undocumented_unsafe_blocks = deny` is not enforced there. I
  checked by hand: all 4 blocks have SAFETY notes. Change: run `cargo clippy --target armv7-unknown-linux-gnueabi -p prysm-h264-rvenc-sys -- -D
  warnings` in that job and make the job required once it is green.
- **C2 could S** — `packaging/debian/build-deb.sh:51-53` (`/bin/sh`, no `pipefail`) pipes the changelog `printf … | gzip -9n`. A failing `date -R
  -d @…` is masked and leaves an empty date. Compute the date first, with `|| exit`. Everything else is strict: `set -euo pipefail` in the bash
  scripts, required-tool checks in `build-ipk.sh`, cleanup traps in `docker/smoke.sh`.

---

## Proposed fix batches

The batches touch disjoint files. **Batch 0 goes first** because the others use its small API additions; after it, batches A–G can run in
parallel. Findings that cross batches name the batch that owns each file.

- **Batch 0 — ports/protocol API (serial, small)**: P1 (add `LightingTarget::{ALL, as_str}`, `Display for SessionState`), P3, P4 (ports `Error`
  only), P10 (Rust side: new error/FailureKind), P13, P15. Files: `crates/ports/**`, `crates/protocol/**`, `tools/export-contracts/src/fixtures.rs`
  (fixtures for the new kind), regenerated `frontend/packages/contracts/generated/*` (codegen only).
- **Batch A — runtime**: P5, P6, P7, P8, P9, P12, P14 (rename `FrameSink::idle` in the ports trait is Batch 0's file, so do it there; A updates
  `crates/runtime/src/forwarder.rs`), P17. Files: `crates/runtime/**`.
- **Batch B — adapters**: A3 (shared resolver: put it in `crates/ports` after Batch 0, or a new module; WLED side `output-wled/src/address.rs`),
  A4, A5, A6, A8, A9, A10, A12, A13, A14, P2 (adapter copies), P4 (adapter `non_exhaustive`), P11 (the adapter `settings.rs` files plus
  `Registry` prefixing in ports — coordinate with Batch 0, or do the `Registry` part in 0), A11 (sys crate comments and unsafe scope). Files:
  `adapters/**` except `adapters/settings-file/**` and `adapters/capture-remote/src/sender.rs`.
- **Batch C — TV hosts and link**: H1, H2 (moves `Video` into webos-link; touches `adapters/capture-webos/src/video.rs` — coordinate with B, or B
  leaves that file to C), H4, H5 (includes `adapters/capture-remote/src/sender.rs`), H6, H9, H10, H3 (the webos `main.rs` side, after E adds the
  helper), P2 (webos-link re-exports). Files: `hosts/webos/**`, `hosts/webos-slim/**`, `hosts/webos-service/**`, `crates/webos-link/**`,
  `adapters/capture-remote/src/sender.rs`.
- **Batch D — desktop hosts, surfaces, tools, settings store**: A7 (rename `prysm_settings_file::FileSettings`; touches `adapters/settings-file/**`,
  `hosts/headless`, `hosts/tui`. The one `hosts/webos/src/main.rs` import is a one-line follow-up for C), P1/P2 call sites in
  `hosts/common`/`surfaces/**`, H3 (add `lan_url` to web-portal and use it in hosts/common), H7, H8, S2–S5. Files: `hosts/common/**`,
  `hosts/headless/**`, `hosts/tui/**`, `surfaces/**`, `tools/export-contracts/src/main.rs`, `adapters/settings-file/**`.
- **Batch E — architecture, at root consolidation**: A1 (`crates/capture/prysm-capture` blocking API plus `adapters/capture-device/**`), A2 (owner
  thread helper; touches capture-device, capture-webos and capture-remote receiver source files, so run after B or fold B's edits to those files
  into E).
- **Batch F — frontend**: F1–F8, plus the frontend side of P10/F4 (after Batch 0's codegen). Files: `v1/frontend/**`, plus a lint step in
  `.github/workflows/v1.yml`.
- **Batch G — CI and packaging**: C1, C2. Files: `.github/workflows/v1.yml` (armv7 job only; coordinate with F's lint step) and
  `v1/packaging/debian/build-deb.sh`.

## Not reviewed, or only skimmed

- Read only by outline or grep, not line by line: `adapters/h264/src/{encoder,decoder,normalize,software,hardware}.rs`,
  `adapters/h264-rvenc-sys/src/gst.rs` (arm-only: SAFETY notes checked, logic not), `adapters/capture-file/src/source.rs` internals,
  `adapters/webos-luna-sys/src/{check,mock}.rs` and `codegen/`, `surfaces/web-portal/src/{preview,access,server,timed_io,response}.rs`,
  `surfaces/tui/src/{input,screen,video}.rs`, `hosts/webos-service/src/{install,launcher,cli,luna,observe,identity}.rs`,
  `frontend/packages/application-{client,state,react}` internals, the `output-*-ui`/`capture-*-ui` panels, `frontend/scripts/*.mjs`, and
  the frontend tests.
- Not run: `cargo test` (the known-flaky test is analysed from the code only), the frontend tests, Miri, and anything needing the webOS SDK
  or hardware. Lints ran for the host target only (macOS), so `cfg(target_os = "linux", target_arch = "arm")` code was not compiled.
- Generated bindings (`webos-luna-sys/src/generated/*`, `frontend/packages/contracts/generated/*`) were excluded on purpose.
