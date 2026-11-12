# Prysm v1 desktop parity audit

> Decision after this audit (2026-11-12): v1 assumes a clean slate, with no detection, migration or upgrade from older installs or settings. D1 (settings converter) and the Debian upgrade path in D13 are dropped.

Status: audit for beads issue prysm-3jx.14 (desktop half), 2026-11-12. Read-only
comparison of the shipping root product with [v1](../../v1/README.md) on branch
`codex/v1-architecture-skeleton`. webOS/TV apps are audited separately; only the
desktop server's side of the TV link is covered here.

## Verdict

v1's runtime core (one engine, one session thread, ordered commands, validate →
persist → apply, stop/close/idle/join with a deadline) is at or above parity, and
both desktop hosts build. The product around it is not: the v1 portal can only
switch lighting on/off and edit a device path, WLED address and remote listen
address; there is no video-file input, no preview, no colour profiles, no
metrics, no logging subscriber, no Debian package or Docker image, and no path
for existing installations' `settings.json` (whose format v1 rejects). The TV
link is a new, incompatible protocol. Root code cannot be deleted until at
least the P1 rows in the [gap list](#gap-list) are done.

## Capabilities

Shipping paths are relative to the repository root; v1 paths to `v1/`.
Status: **present**, **partial**, **missing**, **changed** (intentionally different).

### Process, CLI and settings file

| Capability | Shipping (where) | v1 status | Note |
| --- | --- | --- | --- |
| Settings path | `--config`, env `PRYSM_CONFIG`, default `settings.json` (`apps/prysm/src/main.rs`) | changed | Required positional argument (`hosts/common/src/args.rs`); no default, no env |
| Listen address | `--listen`/`PRYSM_LISTEN`, default `0.0.0.0:8787` | present | Same default; CLI only |
| Host allowlist | `--allowed-host`/`PRYSM_ALLOWED_HOSTS`, default `localhost:8787,127.0.0.1:8787` (`apps/prysm/runtime/src/config.rs`) | changed | Replaced by `--allow-origin` (Origin/CORS policy); `Host` not checked |
| Metrics listener | `--metrics-listen`/`PRYSM_METRICS_LISTEN` | missing | See D10 |
| Validate-only command | `prysm check` | missing | D15 |
| Disable portal, autostart | — | added | `--no-portal`, `--start` are new in v1 |
| Logging | `tracing` to stderr, `RUST_LOG` filter | missing | v1 installs no subscriber: `tracing` events from `prysm-capture`/`prysm-processor` are dropped (D11). Hosts print state changes with `eprintln!` |
| Exit codes | 0 / 1 (anyhow) | present | 0 clean, 1 failure, 2 bad arguments |
| Settings format | `{calibration, source, input:{type,…}, output:{type,url,leds}, processing}` (`crates/common/prysm-contract/src/settings.rs`) | changed | `{capture:{kind,…}, output:{kind,…}, processing}` (`crates/ports/src/settings.rs`); no converter (D1) |
| Missing file | Defaults; portal opens setup | present | Defaults; no setup flow (D6) |
| Malformed file | Fails at start | present | Fails at start |
| Parsable but invalid values | Fails at start | changed | Kept and reported in `Snapshot.failure` |
| Atomic save | `atomic-write-file` | present | Temp file, fsync, rename, dir fsync; keeps permissions, new file `0600` (`adapters/settings-file`) |
| Revision | Random per-process start (`ConfigRevision::new_session`), optimistic check | partial | Starts at 0 every process: a draft from before a restart can match (D19) |
| Processing validation | Bounded newtypes; `sample_density` 1..`u32::MAX` | partial | Same ranges except `sample_density` is `u16` (1..65535) |

### Capture

| Capability | Shipping (where) | v1 status | Note |
| --- | --- | --- | --- |
| V4L2 (Linux), YUYV/RGB3/BGR3 negotiation, no MJPEG | `crates/capture/prysm-capture/src/linux.rs` | present | Reused via `adapters/capture-device`; same crate |
| AVFoundation (macOS), YUYV | `prysm-capture/src/macos.rs` | present | Reused; unverified on hardware (prysm-3jx.22) |
| Resolution / fps | Required, ≤3840×2160, fps ≥1 | present | Optional, default 640×360 @ 30 |
| Device enumeration / listing | — | n/a | Neither has it; user types path or AVFoundation unique ID |
| Stalled V4L2 stream restart, release watchdog | `linux.rs`, `device.rs` | present | Reused; adapter adds 10 s ready / 3 s release deadlines |
| Disconnect / hot-plug retry | No automatic retry; pipeline `Failed` | present | Session ends with failure; no retry (same) |
| Video file playback (FFmpeg 9+, HDR→sRGB, paced, once) | `apps/prysm/runtime/src/video.rs` | present | `adapters/capture-file`, kind `file`; `repeat` loops; the end of the file stops lighting without a failure and sets the target `off` |
| Test patterns | `video_test.sh` (v4l2loopback) | n/a | v1 has runtime fakes (`testkit`) only |
| TV receiver endpoint | `/api/capture` WebSocket on the portal port (`prysm-capture/src/receiver.rs`) | changed | Raw TCP on its own port, kind `remote` (`adapters/capture-remote`) |
| TV receiver lifetime | Always listening; reports idle/busy to TVs | changed | Binds only while a session runs; the slim worker reconnects every second and reports "waiting for the receiver" without exiting, so frames resume about a second after lighting starts (D18, [rationale](prysm-v1-simplified-runtime.md#engine)). The TV keeps capturing while it waits |
| Capture-settings negotiation | Server sends resolution/fps to the TV | missing | TV decides; server has no say (D18) |
| Codecs | Raw, LZ4, H.264 (openh264 decode) (`prysm-capture/src/webos.rs`) | partial | Raw RGB24/BGR24/YUYV/NV12 and H.264 (OpenH264 decode to NV12), wire version 3: the receiver's settings request size, rate, menus and encoding (prysm-3jx.12.2, prysm-3jx.63) |
| Multiple TVs | FIFO queue, `busy` reply, 16 clients | changed | Newest connection replaces the current one |
| Receiver access control | Host allowlist on the HTTP port | missing | None; any LAN host can inject frames |
| "Waiting for frames" status | `input unavailable; waiting for frames` (`pipeline/worker.rs`) | present | `State.input`: fps and `receiving`, sampled once a second; TUI shows "waiting for frames" or the rate (D17) |

### Processing and LED geometry

| Capability | Shipping (where) | v1 status | Note |
| --- | --- | --- | --- |
| Brightness, shadow lift, smoothing, sample density, spatial reach, black-bar removal | `prysm-contract/src/processing.rs` | present | Same defaults from `prysm_core::Config` |
| Live processing changes | Applied without restart | present | Applied at next frame boundary |
| Colour correction from active profile | `Settings.calibration` → `Config.correction` | missing | `Processing::config` uses default correction (D7) |
| LED counts per edge, zero skips edge, total 1..65535 | `prysm-contract/src/resources.rs` | present | `adapters/output-wled/src/settings.rs` |
| Wiring order (bottom-left, clockwise) | Fixed in `prysm-renderer-wled` | present | Same renderer. Neither supports offset, direction or gaps |
| Quiet-source refresh | Pipeline resends | present | Last colours rewritten every 500 ms |

### WLED

| Capability | Shipping (where) | v1 status | Note |
| --- | --- | --- | --- |
| DDP RGB over UDP 4048 | `crates/rendering/prysm-renderer-wled` | present | Reused |
| Address form | HTTP(S) base URL: hostname, port, `https` | partial | IP literal only, HTTP port fixed 80, no TLS (D2) |
| Live/off/effects JSON commands | `control/backend.rs` `WledCommand` | present | `adapters/output-wled/src/output.rs`; v1 also sends `live:true`, `tt:0` |
| Off after final DDP packet | `control/switching.rs` | present | Session idles after loop exit |
| "WLED effects" (native) | `Source::Wled` | partial | Engine `DeviceNative`; TUI key `n`; no portal button (D5) |
| Failed-start rollback to previous state | `switching.rs` | changed | v1 leaves output off and reports failure |
| 12 s start confirmation (frames + WLED live) | `switching.rs` | changed | Start succeeds once source is ready and WLED acknowledged |
| WLED state observation (WebSocket push, 30 s HTTP resync, backoff) | `control/observation.rs` | missing | Takeover from the WLED app goes unnoticed (D16) |
| Discovery, multiple devices | — | n/a | Neither |
| Null output | — | added | Kind `null` |

### Control API

| Capability | Shipping (where) | v1 status | Note |
| --- | --- | --- | --- |
| Read settings | `GET /api/settings` (`crates/common/prysm-api`) | changed | Part of `GET /api/v1/state` |
| Write settings | `PUT /api/settings` → `SettingsView` | changed | `PUT /api/v1/settings` → `{revision}` |
| Lighting state | `GET /api/state` (409 until configured) | changed | `GET /api/v1/state`, always 200 |
| State stream | — (polling) | added | `GET /api/v1/state/stream` (SSE) |
| Set target | `PUT /api/state` `{power, source}` → 202 + operation id | changed | `PUT /api/v1/target` → 204 when applied |
| Calibration | `POST /api/calibration` | missing | prysm-3jx.11 |
| Preview | `/api/preview` WebSocket | missing | prysm-3jx.16 |
| TV capture | `/api/capture` WebSocket | changed | Separate TCP port |
| Health/readiness | none | n/a | Neither; state endpoint serves as probe |
| CSRF / origin policy | Host allowlist, `tower-http` CSRF, Origin = Host | present | Origin policy with CORS opt-in (`surfaces/web-portal/src/access.rs`) |
| Security headers | `no-store`, `nosniff`, CSP with `frame-ancestors 'none'` (`control/http.rs`) | partial | No CSP (D20) |
| Body limit | 64 KiB | changed | 1 MiB; plus connection/stream/timeouts limits |
| Error body | `{code, message}` | changed | `{kind, message, field?, current?}` |
| OpenAPI codegen | `cargo xtask codegen api` | present | `tools/export-contracts`, `npm run codegen` |

### Browser control UI

Shipping: `crates/web/prysm-web/control-ui`. v1: `frontend/packages/web-portal`
and `frontend/hosts/headless/web-portal`.

| Screen / control | v1 status | Note |
| --- | --- | --- |
| Lights on/off | present | Two buttons |
| Screen sync vs WLED effects selector, Open WLED link | missing | D5 |
| Setup wizard (video source, lights, review) | missing | D6 |
| Capture kind select, device path | present | |
| Device resolution and fps, file path | missing | D4, D3 |
| Remote listen address | added | |
| WLED address | present | IP only |
| LED counts per edge, wiring diagram (`LedLayout.tsx`) | missing | D4 |
| Picture settings (six processing controls) | missing | D4 |
| Output kind `null` selector | missing | D4 |
| Save & apply / Discard, conflict review | partial | Per-section save; local stale check, no draft review |
| Colour profile picker, rename, delete | missing | D7 |
| Preview (frame, crop outline, edge strips) | missing | prysm-3jx.16 |
| Technical details (frames processed, revisions, WLED live state) | partial | Target, session, revision, failures only |
| Calibration (phone) and TV relay page | missing | prysm-3jx.11 |

### Calibration end to end (shipping)

TV app opens Calibrate Colors → QR with pairing fragment → phone joins → server
issues TV/phone capabilities (2 min pairing, 30 s reconnect, 2 s
uncorrected lease) → TV plays MSE/MP4 patterns through `/calibration-tv.html`
relay → phone adjusts warmth, tint, midtones, intensity, six hues → save
profile into `settings.calibration` (≤32 profiles) → active profile applied in
processing. Code: `apps/prysm/runtime/src/control/calibration.rs`,
`crates/common/prysm-contract/src/calibration.rs`, `crates/common/prysm-core/src/correction.rs`,
`crates/web/calibration-patterns.json`. v1: none of it; tracked as prysm-3jx.11.
Profile *application* (D7) is separable and should not wait for the session.

### Observability

| Capability | Shipping (where) | v1 status | Note |
| --- | --- | --- | --- |
| Prometheus exporter, role `Prysm` | `crates/common/prysm-metrics`, `--metrics-listen` | missing | Capture and renderer still call `prysm_metrics`, but nothing starts the exporter (D10) |
| Pipeline/preview/receiver metrics | `pipeline/worker.rs`, `preview.rs`, `receiver.rs` | missing | Would need session instrumentation |
| Grafana/Prometheus stack, dashboard self-check | [observability](../../observability/README.md) | missing | Dashboard queries assume shipping metric names |
| Trace stats (`TraceStats`) | `prysm-metrics/src/trace_stats.rs` | partial | Logged through `tracing`, which v1 drops |

### Packaging, deployment and release

| Capability | Shipping (where) | v1 status | Note |
| --- | --- | --- | --- |
| Debian package `prysm-led` (cargo-deb, `/usr/bin/prysm`) | `apps/prysm/Cargo.toml`, `packaging/debian` | missing | D13 |
| systemd unit: user `prysm`, group `video`, `/etc/prysm`, SIGINT, 90 s stop, hardening | `packaging/debian/prysm.service` | missing | D13 |
| sysusers, purge removes settings, upgrade test | `prysm.conf`, `postrm`, `tests/test_upgrade.py` | missing | D13 |
| Docker image (non-root 10001, `/config`, port 8787, multi-arch) | `Dockerfile`, `compose.yaml` | missing | prysm-3jx.18 |
| TrueNAS compose and guide | `packaging/docker/compose.truenas.yaml`, `docs/users/truenas.md` | missing | Uses `PRYSM_ALLOWED_HOSTS`, which v1 drops |
| Image smoke test | `packaging/docker/smoke.py` | missing | Calls `/api/settings` |
| xtask setup/check/codegen/package | `xtask/src/main.rs` | partial | npm scripts and cargo commands only |
| CI and GHCR release on tag (version from `apps/prysm/Cargo.toml`) | `.github/workflows/check.yml` | missing | `.github/workflows/v1.yml` is stale: formats and runs removed crates (`prysm-contract`, `prysm-flow`, `prysm-assemblies` examples) (D14) |

### Shutdown and lifecycle

| Capability | Shipping | v1 status | Note |
| --- | --- | --- | --- |
| Start idle after restart | Yes | present | Plus `--start` |
| SIGINT/SIGTERM drain | Controller drain, 5 s HTTP grace | present | 10 s session deadline, 5 s portal grace |
| Stop ordering, WLED compensation on every exit | Yes | present | `crates/runtime/src/session.rs` |
| Hung stop | Fine-grained cleanup states | changed | Single `Faulted`, restart required |
| Surface failure isolation | — | added | Portal/TUI failure leaves lighting running |

### Tests and tooling worth keeping

| Item | Where | Disposition |
| --- | --- | --- |
| Processor pipeline tests, black-bar tests, examples | `crates/processing/prysm-processor/tests` | Keep with crate |
| V4L2 recovery test, capture smoke example | `crates/capture/prysm-capture` | Keep |
| WebSocket receiver health test | `prysm-capture/tests/receiver_health.rs` | Dead with the `remote` feature |
| Controller scenarios (output change does not power new lights, stop with WLED unreachable, conflicting saves) | `apps/prysm/tests/controller.rs`, `control/ready.rs` | Port the still-wanted ones to `crates/runtime/tests` (D22) |
| Docker smoke, Debian upgrade test | `packaging/` | Rewrite with D12/D13 |
| Black-bar replay harness, sampling A/B | [research](../../research/README.md) | Keep |
| Calibration pattern generator | `scripts/calibration-patterns.py` | Keep only if calibration is ported |

## Intentionally changed or dropped in v1

Items marked ⚑ look like side effects rather than recorded decisions.

| Change | Evidence |
| --- | --- |
| No Host allowlist; Origin/CORS policy instead; DNS rebinding accepted | `surfaces/web-portal/HTTP-CONTRACT.md#access` |
| No token or login on portal or receiver (same as shipping portal) | Contract, `capture-remote/README.md` |
| Kind-tagged adapter settings, untyped until the factory decodes them | `crates/ports/src/settings.rs` |
| No operation IDs; commands answer when applied (204), SSE state stream | `HTTP-CONTRACT.md` |
| `/api/v1/*` routes; error body `{kind,…}` | `surfaces/web-portal/src/routes.rs` |
| Single `Faulted` state replaces cleanup-pending states | Simplified runtime plan |
| Terminal preview surface and `null` output | `surfaces/tui`, `crates/runtime/src/null.rs` |
| Invalid loaded settings no longer stop startup | `crates/runtime/src/engine.rs` |
| ⚑ Settings file is a positional argument; no `PRYSM_*` environment variables | `hosts/common/src/args.rs` |
| ⚑ TV protocol: raw TCP on a second port, TV picks capture size, newest sender wins, listener only while lighting | `adapters/capture-remote` |
| ⚑ No rollback to the previous lighting state after a failed start; no 12 s confirmation | `engine.rs` `start` |
| ⚑ Selected mode (Screen sync vs WLED effects) not persisted | Shipping `Settings.source` |
| ⚑ WLED address is an IP literal; HTTP port fixed at 80 | `adapters/output-wled/src/settings.rs` |
| ⚑ Revision restarts at 0 per process | `Revision` in `crates/ports/src/settings.rs` |
| ⚑ `sample_density` narrowed from `u32` to `u16` | `Processing` |

## Gap list

Sizes: S < 200 lines, M < 800, L more. P1 must land before root code is deleted.

| ID | Title | What is missing | Where in v1 | Size | Depends on | Priority |
| --- | --- | --- | --- | --- | --- | --- |
| D1 | Convert shipping settings files to v1 format | v1 rejects every existing `settings.json` (`input`, `source`, `calibration`, `url`). Map `input.device` → `capture{kind:device,path,width,height,fps}`, `output.wled.url` → `address`, processing 1:1 (clamp density), profiles per D7; report what cannot map (`file` before D3, `webos`) | `hosts/common` subcommand or `tools/` | M | D2, D7 | P1: Debian/Docker/TrueNAS installs keep settings across upgrade |
| D2 | Accept WLED hostnames and a custom HTTP port | `address` is `IpAddr`; shipping accepts `http://wled.local:8080/`. Resolve at open; keep DDP 4048 | `adapters/output-wled` | S | — | P1: saved URLs are commonly mDNS names |
| D3 | Add an FFmpeg video-file source | Kind `file` `{path,width,height,fps}`: FFmpeg 9+ subprocess, HDR→sRGB filter, PTS pacing, play once, kill and reap on close | new `adapters/capture-file`, registered in both hosts | M | — | P1: stated `prysm-tui` target |
| D4 | Expose processing, LED layout and capture mode in portal | Six processing controls, edge counts with wiring diagram, device width/height/fps, output kind `null`/`wled` selector | `frontend/packages/*-ui`, `web-portal` | M | — | P1: otherwise only JSON editing configures production |
| D5 | Add WLED effects mode and Open WLED to portal | Engine has `DeviceNative`; portal lacks the button and the link to WLED's UI | `frontend/packages/web-portal` | S | — | P1: daily control in shipping |
| D6 | Add guided first-run setup to portal | Shipping shows Setup when input/output are missing (source → lights → review) | `frontend/hosts/headless` | M | D4 | P2: setup possible via panels once D4 exists |
| D7 | Persist and apply colour-correction profiles | Profiles (≤32, id, name, correction) and active id in settings; `Processing::config` passes correction; portal picker, rename, delete. Refines prysm-3jx.11 | `crates/ports` settings, frontend | M | — | P1 if calibration is kept (Q1): migrated users' colours change silently |
| D8 | Port the calibration session (desktop side) | `/api/calibration`, TV/phone capabilities, leases, pattern relay page, phone UI. Tracked prysm-3jx.11 | `surfaces/web-portal`, `crates/runtime` | L | D7, TV UI | P2: needs the v1 TV UI anyway |
| D9 | Serve browser preview with crop and edge strips | Demand-driven JPEG frames, viewport from the processor, sRGB strips, 8 fps, 4 viewers. `Preview` slot lacks the viewport. Tracked prysm-3jx.16 | `crates/runtime` session, `surfaces/web-portal`, frontend | M | — | P1: the user tuning guide depends on it |
| D10 | Restore opt-in Prometheus metrics | `--metrics-listen`, start `prysm_metrics`, session/frame metrics, update dashboard and catalog | `hosts/common`, `crates/runtime` | M | Q6 | P2: diagnostics only |
| D11 | Install a logging subscriber with RUST_LOG | Capture/processor `tracing` events are dropped; add stderr subscriber in hosts (TUI: to a file or off while full-screen) | `hosts/common` | S | — | P1: production failures otherwise lose their cause |
| D12 | Build the prysm-headless Docker image | Image from `v1/frontend` + v1 workspace, `CMD ["/config/settings.json"]`, user 10001, `/config`, 8787 and remote port, compose and TrueNAS compose without `PRYSM_ALLOWED_HOSTS`, smoke test on `/api/v1/state`. Tracked prysm-3jx.18 | repo root `Dockerfile`, `packaging/docker` | M | D11, D14 | P1 |
| D13 | Package prysm-headless as a Debian service | cargo-deb metadata, unit with positional settings and `video` group, sysusers, purge, upgrade from `prysm-led` (binary rename, settings via D1), upgrade test | `v1/hosts/headless`, `packaging/debian` | M | D1 | P1: Raspberry Pi is the documented primary install |
| D14 | Fix v1 CI and add release publishing | `v1.yml` references removed crates and examples; add tag check against v1 version, multi-arch image publish, frontend build before host build | `.github/workflows` | S | — | P1: v1 currently has no working CI gate |
| D15 | Add settings check command | Validate a settings file against registered adapters without opening devices | `hosts/common` | S | — | P3: convenience |
| D16 | Observe WLED state and report takeover | Shipping watches WLED WebSocket/HTTP for power, live, override; v1 cannot tell the user that WLED left live mode | `adapters/output-wled` + snapshot field | M | — | P2 |
| D17 | Report input availability and frame rate | `waiting for frames`/no sender connected, frames processed or fps in `State` | `crates/ports` snapshot, session | S | — | P2: remote source otherwise looks healthy with no TV |
| D18 | Keep the remote receiver listening while lighting is off | Bind independent of sessions, report connected sender, optional capture-size hint to the TV. Refines prysm-3jx.12 (with compression, reconnect) | `adapters/capture-remote`, runtime | M | TV link design | P1: TV link is a stated headless target |
| D19 | Seed settings revision per process | Start from a random value as shipping does, so pre-restart drafts conflict | `crates/runtime` engine | S | — | P2: lost update across restart |
| D20 | Add CSP and frame-ancestors headers to portal | Shipping sends CSP `default-src 'self'`, `frame-ancestors 'none'` | `surfaces/web-portal/src/response.rs` | S | — | P2: clickjacking defence |
| D21 | Rewrite user and developer docs for v1 | `docs/users/*` (quickstart, configuration, preview, debian, docker, truenas, troubleshooting), `docs/developers/{README,architecture,control-api,frontend,monitoring,packaging,debugging,error-handling}`, `CLAUDE.md`; fix `settings-file` README WLED example (`host`/`port`) | `docs/` | L | most rows | P1: guides must describe current code |
| D22 | Port still-wanted controller scenarios as runtime tests | Output change must not power new lights; stop with WLED unreachable; settings conflicts | `crates/runtime/tests` | S | — | P2 |
| D23 | Verify desktop hosts on real hardware | V4L2 card, macOS camera permission, WLED live/off/native with v1's `live:true`, Docker device mapping. Tracked prysm-3jx.22 | — | M | D2, D12 | P1 |

## Root code after the apps are deleted

| Root path | Fate | Reason |
| --- | --- | --- |
| `crates/common/prysm-core` | Move into v1 | Used directly |
| `crates/processing/prysm-processor` (+ tests, examples, `TODO.md`) | Move into v1 | Used directly |
| `crates/capture/prysm-capture` | Move, minus `remote` feature | `receiver.rs`, `webos.rs`, `tests/receiver_health.rs`, `axum`/`yuv`/`prysm-webos-protocol` deps are only used by `apps/prysm` |
| `crates/rendering/prysm-renderer-wled` | Move into v1 | Used by `output-wled` |
| `crates/common/prysm-metrics` | Move if D10, else strip calls | Pulled in by capture and renderer |
| `crates/capture/prysm-webos-protocol` | Delete unless the TV audit keeps H.264/LZ4 | Only root apps and the `remote` feature use it |
| `crates/common/prysm-contract`, `prysm-api`, `crates/web/prysm-web` | Delete | Root API and UI |
| `crates/web/calibration-patterns.json`, `scripts/calibration-patterns.py` | Keep only with D8 | |
| `xtask`, `Dockerfile`, `compose.yaml`, `packaging/debian`, `packaging/docker` | Replace (D12–D14) | |
| `observability/` | Keep with D10, else delete | |
| `scripts/check-doc-links.py`, `research/` | Keep | |

Already dead or stale: `.github/workflows/v1.yml` (removed crates);
`packaging/debian/preinst` (retires a `prysm-web.service` proxy from an older
package); the WLED example in `v1/adapters/settings-file/README.md` and the
`Settings` doc comment (`host`/`port` fields that `output-wled` rejects).

## Open questions

| # | Decision | Recommendation |
| --- | --- | --- |
| Q1 | Keep colour profiles and calibration on the desktop product? | Keep profiles (D7) now so migrated colours do not change; port the session (D8) with the v1 TV UI |
| Q2 | Offer video-file input in `prysm-headless` too, or only `prysm-tui`? | Register in both; it is one adapter. Leave FFmpeg out of the Docker image as today |
| Q3 | WLED over HTTPS? | Drop HTTPS (WLED serves plain HTTP); accept hostnames and port (D2) |
| Q4 | Accept no Host allowlist (DNS rebinding) on a LAN portal with no login? | Accept, as the contract argues; document binding to one interface |
| Q5 | Restore `PRYSM_*` environment variables for Docker/systemd? | No: put arguments in `CMD`/`ExecStart`; add only if a NAS UI needs env-only config |
| Q6 | Keep Prometheus metrics and the Grafana stack? | Keep opt-in exporter (P2); delete `observability/` if nobody uses it |
| Q7 | Settings migration: automatic on load, one-shot command, or manual guide? | One-shot command, run by Debian `postinst`; documented for Docker/TrueNAS. Never rewrite silently on load |
| Q8 | Restore failed-start rollback and the 12 s confirmation? | Drop rollback (off on failure is clearer); cover the user need with D16/D17 |
| Q9 | Remote receiver only while lighting is on, newest TV wins, no auth? | Listen always (D18) so the TV can show "connected"; keep newest-wins; add a shared secret only if Q4 changes |
| Q10 | Persist the last mode (Screen sync vs WLED effects)? | No; `--start` covers autostart. Revisit if users ask to resume after reboot |
