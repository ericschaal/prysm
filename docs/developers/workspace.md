# Workspace

Prysm is one Cargo workspace at the repository root, with every crate under
`crates/` in one folder per layer, plus the npm workspace in
[`frontend/`](../../frontend/README.md). Concurrency lives in
[`prysm-runtime`](../../crates/engine/runtime/src/lib.rs): one engine task owns settings
and intent, one session thread owns a source, the processor and an output.
Adapters are single-trait objects with no channels or state machines. Design
and rationale: [architecture](architecture.md) and its
[decision](decisions/2026-11-12-one-runtime-plain-adapters.md).
The implementation before 1.0 is in the tag `pre-1.0-consolidation`.

## Layout

| Directory | Crates | Role |
| --- | --- | --- |
| `crates/engine/` | [`prysm-core`](../../crates/engine/core/src/lib.rs), [`prysm-processor`](../../crates/engine/processor/src/lib.rs), [`prysm-ports`](../../crates/engine/ports/src/lib.rs), [`prysm-runtime`](../../crates/engine/runtime/src/lib.rs), [`prysm-protocol`](../../crates/engine/protocol/src/lib.rs), [`prysm-webos-link`](../../crates/engine/webos-link/src/lib.rs), [`prysm-version`](../../crates/engine/version/src/lib.rs) | Frame and colour types, colour correction; edge sampling and smoothing; ports and settings; engine, session, builder; JSON wire types; TV supervisor ↔ worker protocol; the product version from `VERSION` |
| `crates/adapters/` | [`capture-device`](../../crates/adapters/capture-device/src/lib.rs) (+ [`-sys`](../../crates/adapters/capture-device-sys/src/lib.rs): V4L2 and AVFoundation), [`capture-file`](../../crates/adapters/capture-file/src/lib.rs), [`capture-remote`](../../crates/adapters/capture-remote/README.md), [`capture-webos`](../../crates/adapters/capture-webos/src/lib.rs) (+ `-sys`), [`h264`](../../crates/adapters/h264/src/lib.rs) (+ [`-rvenc-sys`](../../crates/adapters/h264-rvenc-sys/src/lib.rs)), [`webos-luna-sys`](../../crates/adapters/webos-luna-sys/src/lib.rs), [`output-wled`](../../crates/adapters/output-wled/src/lib.rs), [`settings-file`](../../crates/adapters/settings-file/README.md) | `FrameSource`, `FrameSink`, `LedOutput` and `SettingsStore` implementations; the Luna bus; H.264 for the remote stream |
| `crates/surfaces/` | [`tui`](../../crates/surfaces/tui/src/lib.rs), [`web-portal`](../../crates/surfaces/web-portal/HTTP-CONTRACT.md) | Ways in and out of a running engine |
| `crates/hosts/` | [`headless`](../../crates/hosts/headless/src/main.rs), [`tui`](../../crates/hosts/tui/src/main.rs), [`common`](../../crates/hosts/common/src/lib.rs) (`prysm-host`); TV: [`webos`](../../crates/hosts/webos/src/main.rs), [`webos-slim`](../../crates/hosts/webos-slim/src/main.rs), [`webos-service`](../../crates/hosts/webos-service/src/lib.rs) | Executables: a builder list plus process options; the TV workers and their supervisor ([LG TV](webos.md)) |
| `crates/tools/` | [`export-contracts`](../../crates/tools/export-contracts/src/main.rs), [`xtask`](../../crates/tools/xtask/src/main.rs), [`testkit`](../../crates/tools/testkit/src/lib.rs) (`prysm-testkit`) | OpenAPI documents for frontend codegen; `cargo xtask`, the build entry point ([develop](README.md#build-and-run)); what the tests share (bounded waits, child processes, schema checks, the desktop host launcher), a dev-dependency only |
| `fuzz/` | [cargo-fuzz targets](../../fuzz/Cargo.toml), their own Cargo workspace | Nightly fuzzing of the size checks and the capture stream's decoders ([run them](debugging.md#fuzzing)) |
| `frontend/` | npm workspace | Portal and TV bundles; see its [README](../../frontend/README.md) |

Contracts: [ownership and generation](contracts.md), [portal HTTP](../../crates/surfaces/web-portal/HTTP-CONTRACT.md),
[UI and host assemblies](ui-assemblies.md).

## Hosts

| Host | Sources | Outputs | Surfaces |
| --- | --- | --- | --- |
| `prysm-headless` (production: Docker, Debian) | `device`, `file`¹, `remote` | `wled`, `null` | web portal |
| `prysm-tui` (local testing) | `device`, `file`¹, `remote` | `wled`, `null` | web portal, terminal |
| `prysm-webos` (full TV package) | `webos` | `wled` | web portal, link to the TV supervisor |
| `prysm-webos-slim` (slim TV package) | TV capture sent to a desktop host's `remote` source; no engine | — | link to the TV supervisor |

The two TV workers run under the supervisor `prysm-webos-service`; see
[LG TV](webos.md). The rest of this section is about the two desktop hosts.

¹ `file` is for development only: it plays a video file through FFmpeg, which
Prysm does not ship. It works only where FFmpeg 9 or newer is installed and on
`PATH` (check with `ffmpeg -version`), such as a developer machine running
from a source checkout. The [Docker image and the Debian package](../../packaging/README.md)
do not include FFmpeg, so `file` does not work there; use `device` or `remote`.

A `file` source plays once (`"repeat": true` loops it); at its end lighting
stops without a failure and the target becomes `off`. The `remote` source
receives the TV's WebSocket on the portal (`/api/v1/capture/stream`), only
while lighting runs, and asks the sender for the stream its settings describe
(size, rate, menus, encoding; see
[capture-remote](../../crates/adapters/capture-remote/README.md)); the TV's slim worker
takes the one Prysm address (`http://host:8787`, or `https://` behind a
reverse proxy), reconnects every second (resolving a host name at each
attempt), reports that it is waiting meanwhile, captures and encodes as
requested, and stops capturing while lighting is off (the receiver closes the
stream as stopped). A second TV that connects takes the stream over; the
first stops sending until its worker restarts (capture stopped and started,
the TV waking, or a new Prysm address). It needs the portal: under `--no-portal` nothing receives. A sender that cannot produce the request (its
encoder failed) refuses it, and lighting stops with its reason. While lighting runs, the state's
`input` gives the measured frame rate and whether frames are arriving; while
none arrive lighting is waiting for video and the output is left to its own
state.

```sh
cargo run -p prysm-headless -- --help
cargo run -p prysm-tui -- crates/hosts/tui/settings.example.json
```

Both take a JSON settings file (examples: [headless](../../crates/hosts/headless/settings.example.json),
[tui](../../crates/hosts/tui/settings.example.json)) and the same process options on the command line:

```text
Usage: prysm-headless [OPTIONS] <settings.json>

Arguments:
  <settings.json>  Settings file

Options:
      --listen <ADDR>          Portal address; port 0 picks one [default: 0.0.0.0:8787]
      --allow-origin <ORIGIN>  Also allow browser pages from ORIGIN (repeatable; 'null': no origin, as the TV app; '*': any)
      --metrics                Also serve Prometheus metrics at /metrics on the portal
      --no-portal              Serve no web portal
      --start                  Request processed lighting at launch (else idle)
      --log-file <PATH>        Append log lines to PATH instead of stderr
      --check-settings         Only validate the settings file for this host's adapters, then exit
  -h, --help                   Print help
  -V, --version                Print version
```

`--no-portal` excludes the portal options, `'*'` excludes other origins and
`--check-settings` takes no other option; bad arguments exit 2. `--metrics`
serves [Prometheus metrics](../../crates/surfaces/web-portal/HTTP-CONTRACT.md#metrics).
`--check-settings` validates the settings file against the host's capture and
output kinds, prints `settings OK` or the invalid field with its causes and
exits 0 or 1, without opening a device or the portal. Log lines (`tracing`
events, including those of the shared capture and processing crates) go to
stderr as `prysm-headless: warn <target>: <message> <fields>`, filtered by
`RUST_LOG` (default `warn,prysm=info`). While the TUI's preview owns the
terminal stderr writes to, log lines go only to `--log-file` and status
lines wait; the state they missed is printed when the preview ends, also
when it failed, and so is the message of a panic meanwhile. The TV workers
log the same way to stderr. The portal is reachable on the LAN
without a password and refuses browser pages from other sites unless allowed;
see [portal access](../../crates/surfaces/web-portal/HTTP-CONTRACT.md#access). Hosts print
the portal URL, start idle, report state changes on stderr and stop on
SIGINT/SIGTERM (the TUI also on `q`). A second SIGINT or SIGTERM while they
shut down exits at once, without finishing the stop; FFmpeg children of the
`file` source are killed then too.

Exit codes: 0 after a clean stop or `--help`/`--version`, 1 if starting or
shutdown failed, 2 for bad arguments, 130 when a second SIGINT or SIGTERM
ended shutdown early (128 + SIGINT, as shells report an interrupted
command); `--check-settings` 0 when the settings are valid, else 1.

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
// A push-style backend runs on an OwnerThread, publishes through its Worker
// and takes the newest frame in next_frame.
// ValidationError fields are relative to the section ("fps"); the registry reports
// them as "capture.fps".
```

An **output** is the same shape with `OutputFactory` and `LedOutput`
(`write(&EdgeColors)`, `idle(self: Box<Self>, IdleMode)`); `idle` runs on every
session exit, so it is also the failure compensation. An output that can ask
its device whether it shows the colours implements `observe()` (default: no
answer); the session calls it after every write, so it must not wait, and the
engine republishes it as `Snapshot::device.issue` once a second. WLED checks
its state about once a second and reports power off, live override, no live
mode, another sender or no answer; Prysm keeps sending and does not switch
WLED back. `brightness()` (read right after `observe()`) reports the device's
own brightness while it scales the colours, as `Snapshot::device.brightness`;
WLED's comes from `bri` in its state and, every 30th check instead of the
state, `if.live.maxbri` and `light.scale-bri` from `/json/cfg`. The session opens the
output at the source's first frame, not at start, and idles it `Native` (the
device's own state) after `VIDEO_GRACE` (3 s) without frames, opening it again
at the next frame; a source that never delivers never takes the device over. Register it with
`.output(MyLeds)` in each host that should offer it.

A **surface** gets an `EngineHandle` (commands, snapshot watch, preview and
colors watches)
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

From the repository root, `cargo xtask check` runs every check in order
([check](README.md#check)). For a quicker loop:

```sh
cargo xtask check rust                                       # one group of it
npm --prefix frontend ci && npm --prefix frontend run bundle   # the portal the hosts embed
cargo test --workspace --all-targets --offline
```

`cargo build` builds the two hosts (the default members). `prysm-host`
(`crates/hosts/common`) embeds the portal bundles for every host that serves one,
including `prysm-webos`; without `frontend/dist`, a debug build serves a "not
built" page and a release build fails ([build](README.md#build-and-run)). Its
feature `desktop`, which only the two desktop hosts enable, adds the adapter
list they share (`desktop_adapters`), so the TV worker builds none of them.
`xtask` depends on no host, so it builds fast on a fresh checkout; CI's
package jobs build with its `dist` command. Every build uses the Rust that
[`rust-toolchain.toml`](../../rust-toolchain.toml) pins. Builds do not
deploy.

## Not done yet

- Metrics on the TV: `/metrics` is opt-in on the desktop hosts only; the full
  TV worker is started by the service without a way to enable it.
- Verification beyond the development setup: other capture hardware, WLED
  devices and TV models.

## Packaging

`prysm-headless` ships as a Docker image and a Debian package with a systemd
service, the TV as two IPKs (full and slim); see [packaging](../../packaging/README.md).
Releases: [releasing](releasing.md).
