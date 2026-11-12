# Prysm v1

A rewrite of Prysm around one runtime and plain blocking adapters. Concurrency
lives in [`prysm-runtime`](crates/runtime/src/lib.rs): one engine task owns
settings and intent, one session thread owns a source, the processor and an
output. Adapters are single-trait objects with no channels or state machines.
Design and rationale: [simplified runtime](../docs/plans/prysm-v1-simplified-runtime.md).
The shipping product is still the root workspace; v1 shares its `prysm-core`
and `prysm-processor`.

## Layout

| Directory | Crates | Role |
| --- | --- | --- |
| `crates/` | [`prysm-ports`](crates/ports/src/lib.rs), [`prysm-runtime`](crates/runtime/src/lib.rs), [`prysm-protocol`](crates/protocol/src/lib.rs), [`prysm-webos-link`](crates/webos-link/src/lib.rs) | Ports and settings; engine, session, builder; JSON wire types; TV supervisor ↔ worker protocol |
| `adapters/` | [`capture-device`](adapters/capture-device/src/lib.rs), [`capture-file`](adapters/capture-file/src/lib.rs), [`capture-remote`](adapters/capture-remote/README.md), [`capture-webos`](adapters/capture-webos/src/lib.rs) (+ `-sys`), [`h264`](adapters/h264/src/lib.rs) (+ [`-rvenc-sys`](adapters/h264-rvenc-sys/src/lib.rs)), [`webos-luna-sys`](adapters/webos-luna-sys/src/lib.rs), [`output-wled`](adapters/output-wled/src/lib.rs), [`settings-file`](adapters/settings-file/README.md) | `FrameSource`, `FrameSink`, `LedOutput` and `SettingsStore` implementations; the Luna bus; H.264 for the remote stream |
| `surfaces/` | [`tui`](surfaces/tui/src/lib.rs), [`web-portal`](surfaces/web-portal/HTTP-CONTRACT.md) | Ways in and out of a running engine |
| `hosts/` | [`headless`](hosts/headless/src/main.rs), [`tui`](hosts/tui/src/main.rs), [`common`](hosts/common/src/lib.rs) (`prysm-host`); TV: [`webos`](hosts/webos/src/main.rs), [`webos-slim`](hosts/webos-slim/src/main.rs), [`webos-service`](hosts/webos-service/src/lib.rs) | Executables: a builder list plus process options; the TV workers and their supervisor ([design](../docs/plans/prysm-v1-webos-supervisor.md)) |
| `tools/` | [`export-contracts`](tools/export-contracts/src/main.rs) | OpenAPI documents for frontend codegen |
| `frontend/` | npm workspace | Portal and TV bundles; see its [README](frontend/README.md) |

Contracts: [ownership and generation](CONTRACTS.md), [portal HTTP](surfaces/web-portal/HTTP-CONTRACT.md),
[UI and host assemblies](UI-ASSEMBLIES.md).

## Hosts

| Host | Sources | Outputs | Surfaces |
| --- | --- | --- | --- |
| `prysm-headless` (production, Docker) | `device`, `file`, `remote` | `wled`, `null` | web portal |
| `prysm-tui` (local testing) | `device`, `file`, `remote` | `wled`, `null` | web portal, terminal |

A `file` source plays once (`"repeat": true` loops it); at its end lighting
stops without a failure and the target becomes `off`. The `remote` source
listens only while lighting runs and asks the sender for the stream its
settings describe (size, rate, menus, encoding; see
[capture-remote](adapters/capture-remote/README.md)); the TV's slim worker
reconnects every second, reports that it is waiting meanwhile, and captures
and encodes as requested. A sender that cannot produce the request (its
encoder failed) refuses it, and lighting stops with its reason. While lighting runs, the state's
`input` gives the measured frame rate and whether frames are arriving; while
none arrive lighting is waiting for video and the output is left to its own
state.

```sh
cargo run --manifest-path v1/Cargo.toml -p prysm-headless -- --help
cargo run --manifest-path v1/Cargo.toml -p prysm-tui -- v1/hosts/tui/settings.example.json
```

Both take a JSON settings file (examples: [headless](hosts/headless/settings.example.json),
[tui](hosts/tui/settings.example.json)) and process options on the command line:
`--listen` (default `0.0.0.0:8787`), `--allow-origin ORIGIN` (repeatable;
`null` for the slim TV app, `'*'` for any), `--no-portal`, `--start` and `--log-file PATH`. Log lines (`tracing`
events, including those of the shared capture and processing crates) go to
stderr as `prysm-headless: warn <target>: <message> <fields>`, filtered by
`RUST_LOG` (default `warn,prysm=info`); while the TUI's preview uses the
terminal they go only to `--log-file`. The TV workers log the same way to
stderr. The portal is reachable on the LAN
without a password and refuses browser pages from other sites unless allowed;
see [portal access](surfaces/web-portal/HTTP-CONTRACT.md#access). Hosts print
the portal URL, start idle, report state changes on stderr, stop on
SIGINT/SIGTERM (the TUI also on `q`) and exit non-zero if shutdown failed.

## Extending

A **source** is one type implementing `FrameSource`, plus a factory:

```rust,ignore
pub struct Camera;                             // the factory
impl SourceFactory for Camera {
    const KIND: &'static str = "camera";       // { "kind": "camera", ... }
    type Settings = CameraSettings;            // serde, deny_unknown_fields
    fn validate(&self, s: &CameraSettings) -> Result<(), ValidationError> { s.validate() }
    fn open(&self, s: &CameraSettings) -> Result<Box<dyn FrameSource>> { Ok(Box::new(Frames::open(s)?)) }
}
// Frames: next_frame(timeout) -> Ok(Pull::Frame(f)), Ok(Pull::Idle) on timeout,
// Ok(Pull::Ended) when finished (lighting stops cleanly); close(self: Box<Self>) releases.
// A push-style backend stores frames in a LatestSlot and takes them in next_frame.
```

An **output** is the same shape with `OutputFactory` and `LedOutput`
(`write(&EdgeColors)`, `idle(self: Box<Self>, IdleMode)`); `idle` runs on every
session exit, so it is also the failure compensation. The session opens the
output at the source's first frame, not at start, and idles it `Native` (the
device's own state) after `VIDEO_GRACE` (3 s) without frames, opening it again
at the next frame; a source that never delivers never takes the device over. Register it with
`.output(MyLeds)` in each host that should offer it.

A **surface** gets an `EngineHandle` (commands, snapshot watch, preview slot)
and a `Shutdown`:

```rust,ignore
async fn log(engine: EngineHandle, shutdown: Shutdown) -> prysm_ports::Result<()> {
    let mut snapshots = engine.snapshot();
    loop {
        tokio::select! {
            () = shutdown.requested() => return Ok(()),
            _ = snapshots.changed() => println!("{:?}", snapshots.borrow().session),
        }
    }
}
// .surface(log) — closures are surfaces; a struct implements Surface. An error
// is reported in Snapshot::surface_failures and lighting continues.
```

## Build and test

From the repository root:

```sh
npm --prefix v1/frontend ci && npm --prefix v1/frontend run build   # hosts include! the bundle
cargo fmt --manifest-path v1/Cargo.toml --all
cargo test --manifest-path v1/Cargo.toml --workspace --all-targets --offline
cargo clippy --manifest-path v1/Cargo.toml --workspace --all-targets --offline -- -D warnings
npm --prefix v1/frontend run codegen:check && npm --prefix v1/frontend test
```

`cargo build --manifest-path v1/Cargo.toml` builds the two hosts (the default
members). Builds do not deploy.

## Not done yet

- TV: IPK packaging for the two packages, the TV UI on the new Luna API, and every
  check that needs the webOS SDK or a TV (the TV binaries build and are tested on
  the development host only).
- Calibration and metrics.
- Verification against real capture hardware, WLED devices and TVs.

## Packaging

`prysm-headless` ships as a Docker image and a Debian package with a systemd
service; see [packaging](packaging/README.md). Releases on `v1.*` tags:
[v1-release.yml](../.github/workflows/v1-release.yml).
