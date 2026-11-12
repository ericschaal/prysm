# Prysm v1 parity audit: webOS TV

> Decision after this audit (2026-11-12): v1 assumes a clean slate, with no detection, migration or upgrade from older installs or settings. W17 (old-package detection) is dropped; full and slim share the app id `io.prysm.tv`, so installing one replaces the other.

Status: audit of 2026-11-12 (beads `prysm-3jx.14`, webOS half). It compares the
shipping TV product in the root workspace with `v1/`, from code. The desktop and
server product is audited separately. Builds on the
[supervisor plan](prysm-v1-webos-supervisor.md), whose §7 checklist this
extends, and the [simplified runtime](prysm-v1-simplified-runtime.md).

## 1. Verdict

The TV back end of v1 is largely written but has never run on a TV. The capture
FFI, the lifecycle limits, the supervisor state machine, setup and uninstall,
and both workers are faithful ports with host tests. The supervisor has more
host tests than the root one. Three things block deletion of the root TV code:
there is no installable package (no IPK, no armv7 build, role and service files
or `appinfo.json`), the TV UI is a fixture-only skeleton, and calibration is
missing. Two v1 changes look accidental and would regress the shipping product.
The full worker lost the NEON NV12 kernel and the Rayon processing, which the
research says are worth 25 → 30 FPS at 640×360 and 17 → 30 FPS at 720p. The slim
worker originally converted NV12 to RGB on the TV and sent RGB24; since W13 it
forwards native NV12 (half the bytes); since `prysm-3jx.12.2` it encodes it
with the hardware H.264 encoder by default (software OpenH264 or raw by
setting), and the receiver pauses it when lighting stops. Everything below the TV
UI is "built, unverified" until `prysm-3jx.22`.

Status values: **present** (code exists and is tested on the host), **built,
unverified** (needs SDK or TV), **partial**, **missing**, **changed** (an
intentional difference, see §3).

## 2. Capability tables

### Install, setup, repair, uninstall

| Capability | Shipping | v1 | Note |
| --- | --- | --- | --- |
| IPK `io.prysm.capture` (app + service dir) | [build-ipk.sh](../../packaging/webos/build-ipk.sh) | built, unverified | [build-ipk.sh](../../v1/packaging/webos/build-ipk.sh) `full`/`slim`: both `io.prysm.tv`, built with the SDK; not installed on a TV (`prysm-3jx.20.2`) |
| Payload hardening: root ownership, `go-w`, no control scripts, links or stray paths | [secure-ipk.py](../../packaging/webos/secure-ipk.py), [test_ipk.py](../../packaging/webos/tests/test_ipk.py) | present | [ipk.py](../../v1/packaging/webos/ipk.py): one allowlist (both flavours share paths), exact service file set, ELF checks; [tests](../../v1/packaging/webos/tests/test_ipk.py) |
| Homebrew Channel `exec setup.sh` from the app (root) | [useCaptureService.ts](../../crates/web/prysm-webos/src/hooks/useCaptureService.ts) `setup()` | partial | `setup` CLI in [cli.rs](../../v1/hosts/webos-service/src/cli.rs), [setup.sh](../../v1/packaging/webos/service/setup.sh) packaged; UI call in [webos-platform](../../v1/frontend/packages/webos-platform/index.ts) `setup()` |
| Role manifests (worker, supervisor), refuse unrelated files | [files.rs](../../apps/prysm-webos-service/src/files.rs) | built, unverified | [install.rs](../../v1/hosts/webos-service/src/install.rs); role/service/permission files in [service/](../../v1/packaging/webos/service/) |
| Absolute `ls-control`/`luna-send` paths (0.1.10 boot failure) | [setup.rs](../../apps/prysm-webos-service/src/setup.rs) | present | `Layout` paths are absolute |
| Bounded rescan and activation, `returnValue` checked | setup.rs (10 s) | present | 10 s `run_bounded` |
| Boot hook `/var/lib/webosbrew/init.d/…` | toggled with autostart | changed | Always installed; runs `setup` |
| Remove setup from the app (keeps settings) | `cleanup` Luna method | present | `uninstall` method; UI missing |
| Migrations `config.json` → `config-v2` → `config-v3`, portal settings seeding | files.rs `load`, `migrate_output` | changed | Dropped; old files left alone |
| Coexistence with the old `io.prysm.capture` install | n/a | missing | Both boot hooks would start capture; only the shared lock arbitrates (W17) |
| Isolated probe identity (`--sandbox`, `--run-for`, probe manifest) | [main.rs](../../apps/prysm-webos-service/src/main.rs) | present | `run --sandbox DIR --run-for S` |

### TV UI (extends [supervisor plan §7](prysm-v1-webos-supervisor.md#7-tv-ui-parity-checklist))

Updated 2026-11-12 (beads `prysm-3jx.21.1`, `.21.3`–`.21.5`). The v1
[TV UI](../../v1/frontend/packages/tv-ui/index.tsx) runs on the
[Luna client](../../v1/frontend/packages/webos-platform/index.ts) of the
supervisor and on the HTTP API of the Prysm that drives the lights. Everything
here is tested on the development host (jsdom, desktop browser at 1920×1080);
nothing has run in a TV browser yet.

| Capability | Shipping | v1 | Note |
| --- | --- | --- | --- |
| Status card, 12 states (connecting, unavailable, waiting for video/TV/Prysm, stopping, paused, ready, capturing, receiver in use, disabled, needs attention) | [capturePresentation.ts](../../crates/web/prysm-webos/src/capturePresentation.ts) | present | [model.ts](../../v1/frontend/packages/tv-ui/model.ts) `tvStatus`; "waiting for Prysm" from the slim worker's `waiting for the receiver` failure; "retrying" shows the failure but no retry time (`State` has none); "receiver in use" dropped with receiver busy |
| Context-sensitive primary button (Set up service / Set receiver address / Save and enable / Enable / Disable / Pause / Allow TV capture) | same | present | Set up service / Set Prysm address / Save and enable / Enable / Disable; full: Start screen sync / Turn lights off (`setEnabled`) |
| Settings rows with saved/unsaved badge and explicit Save | [Settings.tsx](../../crates/web/prysm-webos/src/components/Settings.tsx) | present | Save and Discard; a field changed elsewhere replaces the edit and says so |
| Instance URL editor: host or IP → `http://host:8787`, https allowed | [CaptureDialog.tsx](../../crates/web/prysm-webos/src/components/CaptureDialog.tsx) | present | `receiver` filled from the remote `capture.listen` port when the instance is an IP; editable (`host:port`, a name or an IP, W12) |
| Capture choice: video + menus / video only | CaptureDialog | changed | In the web portal, not the TV UI (neither package has a TV capture dialog; `prysm-3jx.63`). Slim: the desktop portal's Remote capture form (`include_ui`), sent to the TV in the stream request. Full: the TV's own portal (`webos` capture) |
| Capture size and rate on the TV | negotiated by the receiver | present | Web portal only. Slim: the desktop portal's Remote capture form (`width`/`height`/`fps`, default 320×180 at 30), sent in the stream request. Full: the TV's own portal |
| Compression choice (LZ4, none, H.264 SW, H.264 HW if built) | CaptureDialog | changed | The desktop portal's Remote capture form (`encoding`: `h264_hardware`, `h264_software`, `raw`); a TV encoder failure stops lighting there with the TV's reason on `capture.encoding`; `State.encoding` reports the one in use. LZ4 dropped |
| Enable with TV | CaptureDialog | present | `preferences.autostart` (full: "Start screen sync with TV") |
| Prometheus metrics checkbox (port 9101) | Settings | changed | No metrics in v1 |
| Service options: set up or repair, diagnostics, remove setup with confirmation | CaptureDialog | present | Homebrew `exec` of `…/services/io.prysm.tv.service/setup.sh`; `uninstall` |
| Diagnostics: power, video, receiver connected, enabled, process running, error or last log | CaptureDialog | present | No "receiver connected" in `State`; `capturing`, worker phase, failure and the 20 log lines instead |
| Portal address and "open on your phone" (full) | [CaptureOverview.tsx](../../crates/web/prysm-webos/src/components/CaptureOverview.tsx) | present | Text, no QR code; the app itself uses `http://127.0.0.1:<port>` |
| Lights on/off on the processing instance | not on TV (portal) | present | Screen sync / WLED effects / Off on the own (full) or remote (slim) portal; full also edits WLED address, LED counts and capture |
| Calibrate colors: QR pairing, relay iframe, MSE pattern player, 17 bundled clips | [Calibration.tsx](../../crates/web/prysm-webos/src/components/Calibration.tsx), [patternStream.ts](../../crates/web/prysm-webos/src/patternStream.ts) | missing | Row shown disabled as "not available in this version yet" (`prysm-3jx.11`) |
| Remote navigation: spatial `data-up/down/left/right`, numeric key codes, magic-remote hover focus, focus restore, caret-safe arrows in inputs | [useRemoteNavigation.ts](../../crates/web/prysm-webos/src/hooks/useRemoteNavigation.ts) | present | [navigation.ts](../../v1/frontend/packages/tv-ui/navigation.ts): two columns and `data-row` groups instead of per-control maps; key codes 37–40, 13, 27, 461 |
| Back: closes dialog one level, then asks "Close Prysm Capture?" | same | present | "Close Prysm?"; the app sets `disableBackHistoryAPI` |
| Drafts survive status updates; stale replies ignored by `pid`/`revision` | useCaptureService.ts | changed | No `pid`/`revision`: replies only confirm, the subscription is the only state; portal dialogs check the section and `stale_revision` |
| 30 s call timeout, 3 s resubscribe, pause on `visibilitychange`/`pagehide` | same | present | Luna and the portal stream |
| Behaviour tests (47 cases, calibration probe) | [app.test.cjs](../../crates/web/prysm-webos/tests/app.test.cjs) | present | [tv.test.jsx](../../v1/frontend/tests/tv.test.jsx) (28 cases, no calibration) |
| Luna contract schema → generated TS types | xtask `codegen` `Contract::Webos`, [service.schema.json](../../crates/web/prysm-webos/service.schema.json) | present | `@prysm/contracts/luna`; replies checked at runtime by `isState` |
| Chromium 53 build (classic script, relative URLs) | [vite.config.ts](../../crates/web/prysm-webos/vite.config.ts) | present | Luna path only; portal control needs webOS 5+ (`AbortController`, streamed `fetch`) and says so on older TVs |
| Slim control of the remote portal from `Origin: null` | — | present | The headless Prysm runs with `--allow-origin null` (exactly `null`, decided in `prysm-3jx.56`); otherwise the TV says how to allow it (Q1) |

### Luna service API

| Shipping `io.prysm.capture.service` | v1 `io.prysm.tv[.slim].service` | Status |
| --- | --- | --- |
| `status {subscribe}` → `config, enabled, receiverConnected, receiverBusy, capturing, portalUrl, senderRunning, stopping, power, error, lastLog, capabilities, videoAvailable, revision, subscribers, pid` | `getState {subscribe}` → `flavor, preferences, enabled, lighting, power, video, worker, capturing, portal, failure, log` | changed: no `receiverConnected`/`receiverBusy`, `revision`, `pid`, `capabilities` |
| `start` / `stop` (deferred reply until done, pause handshake) | `setEnabled {enabled}` (reply when accepted) | present |
| `setConfig` (verifies the instance first, 12 s) | `setPreferences` (validates only) | changed |
| `cleanup` | `uninstall` (also exits the service) | present |
| App-id check before parsing; one change at a time | app-id check before parsing; no in-progress lock | present |
| `inbound: ["*"]` role, public api permission file | [supervisor.role.json](../../v1/packaging/webos/service/supervisor.role.json), [supervisor.api.json](../../v1/packaging/webos/service/supervisor.api.json) | built, unverified |

### Capture

| Capability | Shipping | v1 | Note |
| --- | --- | --- | --- |
| VTCapture FFI, C++ exception boundary, dlopen of LG libraries | [native/vtcapture.rs](../../apps/prysm-webos-capture/src/native/vtcapture.rs) | built, unverified | [capture-webos-sys](../../v1/adapters/capture-webos-sys/src/vtcapture.rs), near line-for-line |
| Luna registration precheck before `vtCapture_create` | [permissions.rs](../../apps/prysm-webos-capture/src/native/permissions.rs) | built, unverified | `prysm_webos_luna_sys::check_registration` |
| Size 2–1920 × 2–1080 even, 1–60 FPS | protocol `ControlMessage` | present | [settings.rs](../../v1/adapters/capture-webos/src/settings.rs) |
| Video + menus (blended, mode 3) / video only (mode 2) | `Source::Both/Video` | present | `include_ui` |
| Status 1 → re-init once per second, 10 s preparation limit | vtcapture.rs | present | `Limits::TV` |
| Status 2 for 2 s → restart tap | vtcapture.rs | present | `pending` |
| 5 s native watchdog; failure published before teardown | supervisor `video_reset_at` + worker | present | in-process watchdog in [source.rs](../../v1/adapters/capture-webos/src/source.rs) |
| Fatal create/permission → process exit | `FatalVideo` | present | `Error::is_fatal` |
| Release certification (4 s) | implicit | present | Unconfirmed release faults the worker |
| `/tmp/prysm-capture.lock`, `O_NOFOLLOW` | [lock.rs](../../apps/prysm-webos-capture/src/lock.rs) | present | Same path, so old and new exclude each other |
| SIGINT/SIGTERM blocked on the capture thread | [shutdown.rs](../../apps/prysm-webos-capture/src/shutdown.rs) | present | blocked in `Lg::open` |
| `videooutput` MAIN readiness, muted counts, context reset | supervisor + `VideoCommand` bytes | present | [observe.rs](../../v1/hosts/webos-service/src/observe.rs) |
| Limited BT.709 NV12 → RGB, portable | `yuv` | changed | No conversion: both workers keep native NV12 frames; the processor and previews sample NV12 in place ([nv12.rs](../../crates/common/prysm-core/src/nv12.rs)) |
| NEON kernel (8 px), parallel row pairs, exact match test | [nv12.rs](../../apps/prysm-webos-capture/src/nv12.rs), [nv12.cpp](../../apps/prysm-webos-capture/local/nv12.cpp) | missing | W1 |
| HDR, DRM, black frames | not handled; capture follows tone mapping ([research](../../research/webos-capture/2026-11-01-hdr-brightness/README.md)) | same | Home without video yields no frames in both |

### Remote (slim) mode

| Capability | Shipping | v1 | Note |
| --- | --- | --- | --- |
| Transport | WS/WSS on the server's HTTP port `/api/capture` ([transport.rs](../../apps/prysm-webos-capture/src/transport.rs)) | changed | Raw TCP to `receiver` (e.g. `:7380`), [sender.rs](../../v1/adapters/capture-remote/src/sender.rs) |
| Instance by hostname, `https`, TLS verified against the TV trust store | `InstanceUrl` | partial | `receiver` is `host:port` (an IP address or a name such as `prysm.local`), checked for syntax when saved and resolved on the TV at every connection attempt; an unresolved name is the receiver wait (`prysm-3jx.26`). No TLS, no verify step: the status is the feedback |
| Versioned hello; `idle`/`busy`/`capture_settings` replies | [control.rs](../../crates/capture/prysm-webos-protocol/src/control.rs) | partial | Wire version 3: hello (version), reply with status and the receiver's stream request (size, rate, menus, encoding); the sender may refuse a request it cannot produce, with a reason; a mismatch is a typed `WireError` on both ends. No `idle`/`busy` |
| Receiver chooses size and FPS (Screen sync) | `CaptureSettings` | present | The desktop's `remote` capture settings; a change restarts lighting and the TV's next connection gets the new request (`prysm-3jx.63`) |
| Capture only while the server wants frames (`Idle`) | `wait_for_capture` | present | The receiver pauses its sender on the open connection (one control byte; sent on `close`, or by `RemoteSource::set_wanted`): nothing is encoded or sent, and the slim worker closes the TV capture until the receiver resumes or, after a pause and close (lighting stopped), connects again; `State.paused` says so (`prysm-3jx.60`). A receiver lost without a pause keeps the capture running |
| Verify instance before saving (`--verify-instance`) | worker CLI | changed | Connect failure = worker failure |
| Receiver busy detection | `Busy` → event, UI state | changed | New connection replaces the old one |
| Frame format on the wire | NV12 (native) | present | Native limited BT.709 NV12, sampled on the receiver without conversion (W13) |
| Codecs: raw, LZ4, OpenH264, GStreamer `rvenc` HW with SW fallback | [encoder.rs](../../apps/prysm-webos-capture/src/encoder.rs), [gst.rs](../../apps/prysm-webos-capture/src/encoder/gst.rs) | built, unverified | [prysm-h264](../../v1/adapters/h264/src/lib.rs) + [h264-rvenc-sys](../../v1/adapters/h264-rvenc-sys/src/lib.rs): the requested `encoding` `h264_hardware` (default), `h264_software`, `raw`; exactly that encoder, no automatic fallback: a failure is reported by the TV (`failure_kind: encoder`) and refused to the receiver (lighting stops with the reason) until the request changes. Receiver decodes to NV12 and asks for a keyframe after a lost picture. LZ4 dropped. Hardware path cross-built only (`prysm-3jx.12.2`) |
| Backpressure | 100 ms frame deadline, drop on overrun, `MissedTick` | partial | 2 s write timeout ends the worker |
| Reconnect | worker exit, supervisor backoff 1–5 s | partial | worker exit, backoff 1–30 s |
| Pacing that skips missed ticks | session.rs `pace` | present | source.rs |

### Embedded (full) mode

| Capability | Shipping | v1 | Note |
| --- | --- | --- | --- |
| Portal on `0.0.0.0:8787` while awake, also when capture is paused | [portal.rs](../../apps/prysm-webos-capture/src/portal.rs) | built, unverified | [hosts/webos](../../v1/hosts/webos/src/main.rs) |
| Host-header allowlist (TV LAN addresses, localhost) | `WebConfig.allowed_hosts` | changed | `Access::AnyOrigin`, no Host check (W11) |
| Advertised LAN address; re-opens when addresses change (5 s check) | portal.rs `lan_addresses` | partial | Computed once at start (W10) |
| Settings store, seeded with a webOS input | `portal-settings.json` | present | `settings.json` seeded with `capture.kind = webos` |
| Portal capture panel (size, FPS, menus) | shared control UI | partial | [capture-webos-ui](../../v1/frontend/packages/capture-webos-ui/index.tsx) edits FPS only, allows 120 (W9) |
| WLED DDP output, idle compensation on stop | prysm-runtime | present | `Wled`, 6 s stop deadline |
| Preview and calibration in the portal | shared portal | missing | v1 portal has neither (`prysm-3jx.11`, desktop audit) |
| Pause without dropping HTTP (`p` → `CapturePaused`) | portal.rs | changed | Lighting Off closes the source |
| Rayon grid and projection (`parallel-projection`), hard-float NEON build flags | [local/Cargo.toml](../../apps/prysm-webos-capture/local/Cargo.toml), [build-tv.sh](../../apps/prysm-webos-capture/local/build-tv.sh) | not needed: the processor samples a bounded pixel lattice and the full package holds 30 FPS up to 1920×1080 on one thread (measured 2026-11-12) | W1 |
| Lighting restored after a session failure | worker restart | present | 1–30 s re-apply in [link.rs](../../v1/hosts/webos/src/link.rs) |

### Power, supervision, diagnostics

| Capability | Shipping | v1 | Note |
| --- | --- | --- | --- |
| `tvpower` subscription, renewed after 5 s, fail closed | [supervisor.rs](../../apps/prysm-webos-service/src/supervisor.rs) | present | [machine.rs](../../v1/hosts/webos-service/src/machine.rs) |
| Remote only in Active; embedded also in Screen Saver | `wanted()` | changed | Both run awake; capture only in Active |
| Manual stop survives standby; cold boot from autostart | same | present | |
| TERM → KILL 5 s; reap before restart | same | present | 8 s window |
| Backoff 1 s doubling to 5 s | `MAX_RETRY_SECONDS` | changed | 1 s doubling to 30 s, reset after 60 s |
| Encoder (5 s) and video-reset (5 s) watchdogs in the supervisor | `encoder_at`, `video_reset_at` | changed | In-process watchdog + 15 s heartbeat silence |
| Parent death: `PR_SET_PDEATHSIG` + ppid check | [worker/native.rs](../../apps/prysm-webos-service/src/worker/native.rs) | present | plus stdin EOF |
| Worker log to UI | last stderr line | present | last 20 lines |
| Prometheus exporter on 9101 (capturer role, codec, wire, stage timers) | [metrics.rs](../../apps/prysm-webos-capture/src/metrics.rs) | changed | none (W16) |
| Frames/FPS visible to the user | metrics only | partial | `capturing` boolean |

### Packaging, build, tests

| Capability | Shipping | v1 | Note |
| --- | --- | --- | --- |
| `cargo xtask package webos [--hardware-h264] [--embedded-prysm]` | [xtask](../../xtask/src/main.rs) | changed | [build-ipk.sh](../../v1/packaging/webos/build-ipk.sh) `full\|slim`, outside xtask; TV feature flags dropped |
| `profile.webos` (fat LTO, strip), `cortex-a9`, soft-float linker with `glibc_polyfills` | [Cargo.toml](../../Cargo.toml), [linker.sh](../../packaging/webos/linker.sh) | present | [v1/Cargo.toml](../../v1/Cargo.toml) `profile.webos`, [env.sh](../../v1/packaging/webos/env.sh) (`cortex-a9`, `-soft-float,+neon`: hardware floats with the soft-float ABI; target pkg-config for the slim worker's GStreamer), [linker.sh](../../v1/packaging/webos/linker.sh) (adds `-ldl`) |
| Licences: Prysm, hyperion, tv-native-apis (+ OpenH264, gstreamer-rs, gtk-rs) | build-ipk.sh | present | Prysm, hyperion, tv-native-apis in `licenses/`; the slim package adds OpenH264, gstreamer-rs and gtk-rs |
| Luna bindgen generator with `--check` | [codegen/main.rs](../../apps/prysm-webos-service/codegen/main.rs) | present | [generate-luna-bindings](../../v1/adapters/webos-luna-sys/codegen/main.rs) (`--features codegen`); `--check` passes against the SDK |
| CI: armv7 qemu tests, SDK IPK artifact, Miri armv7 + x86-64 | [check.yml](../../.github/workflows/check.yml) | missing | [v1.yml](../../.github/workflows/v1.yml) names crates that no longer exist |
| Native smoke on the TV (read-only, `--supervisor`, `--capture`) | [native-smoke.py](../../packaging/webos/tests/native-smoke.py) | built, unverified | [native-smoke.py](../../v1/packaging/webos/tests/native-smoke.py): probe `io.prysm.tv.probe`, `run --sandbox`; dry-run only, never run on a TV |
| Research harnesses: on-TV replay, packet observer, lifecycle probes | [on-TV pipeline](../../research/webos-capture/2026-11-04-on-tv-pipeline/README.md) | n/a | Keep in `research/`; reuse for W1 and W18 |

## 3. Intentionally changed or dropped in v1

| Change | Decided where | Possibly unconsidered? |
| --- | --- | --- |
| Two packages; no in-app full/slim switching, no `location`, no `capabilities` | Plan §1.1 | No |
| Portal on the LAN without a token | Plan §1.2 | Partly: `AnyOrigin` with no Host check also lets any website in a LAN browser drive the portal (W11) |
| Slim streams continuously; only-capture-when-wanted is gone | Plan §1.3 | Addressed (`prysm-3jx.12.2`, `prysm-3jx.60`): while the server's lights are off nothing is captured, encoded or sent |
| LZ4 dropped; hardware H.264 (default), OpenH264 and raw are an explicit setting without automatic fallback | Q2 decision, `prysm-3jx.12.2` | The hardware encoder's DMA, normalisation and first-picture rules are ported; its TV qualification has to be repeated for v1 |
| Receiver chooses size/FPS → TV preferences | Plan §6 | Partly: the server's capture settings and the TV's now drift apart |
| WS/WSS on the server port → raw TCP on a second port, no TLS | [capture-remote README](../../v1/adapters/capture-remote/README.md) | Yes: https instances and firewalls with one open port stop working (host names work again, `prysm-3jx.26`) |
| Receiver busy and instance verification dropped | Plan §2 | Partly: two TVs now steal the receiver silently |
| Prometheus metrics on the TV dropped | Plan §7 "later" | Partly |
| Config migrations and old Luna names dropped | Plan header | Yes for upgrade: nothing removes the old hook (W17) |
| Boot hook always installed | Plan §2 | No |
| Backoff cap 5 s → 30 s | Plan §2 | No; repeated wake failures recover more slowly |
| Exit confirmation on Back | v1 TV UI skeleton | Yes |

## 4. Gap list

| ID | Title | What is missing | Where in v1 | Size | Needs | Depends on | Priority |
| --- | --- | --- | --- | --- | --- | --- | --- |
| W1 | Restore NEON conversion and parallel processing on TV | NEON `prysm_nv12_rows` kernel + Rayon row bands with the exhaustive Y/U/V equality test; `parallel-projection` on `prysm-processor` for the full worker; `+neon,-soft-float` Rust flags; `yuv` Rayon fallback | `capture-webos` backend (+`build.rs`), `hosts/webos` Cargo features, build script | M | SDK, TV | W2 | Done differently (prysm-3jx.24): hard-float NEON flags plus bounded sampling; no NEON kernel, no Rayon |
| W2 | Add armv7 build profile and CI for v1 TV crates | `profile.webos`, linker wrapper, qemu `cargo test --target armv7`, fix stale `v1.yml` crate names, Miri for `webos-luna-sys` | `v1/Cargo.toml`, `.github/workflows/v1.yml` | S | dev host | — | P1: refines `prysm-3jx.20`. Profile and wrapper done (`prysm-3jx.20.2`); CI open. |
| W3 | Build and harden the two v1 IPKs | `appinfo.json` + icon per app, `supervisor.role.json`, `worker.role.json`, `.service`, client/api permission files, `setup.sh`, licences, two `secure-ipk.py` allowlists and tests, xtask `package webos --slim` | `packaging/webos`, `xtask` | M | SDK | W2 | P1: refines `prysm-3jx.20`. Done on the SDK (`prysm-3jx.20.2`): one allowlist, `build-ipk.sh full\|slim`, no xtask. |
| W4 | Port bindgen generator and SDK check | `generate-luna-bindings --check` for `webos-luna-sys` from SDK headers | `v1/adapters/webos-luna-sys` (tool) | S | SDK | W2 | P2: refines `prysm-3jx.20`. Done (`prysm-3jx.20.3`); `--check` passes. |
| W5 | Port native smoke test to v1 names | `native-smoke.py` with `io.prysm.tv[.slim].probe`, `run --sandbox`, capture probe | `packaging/webos/tests` | S | TV | W3 | P1: refines `prysm-3jx.20`. Written, dry-run only (`prysm-3jx.20.4`); needs the TV. |
| W6 | Implement the TV Luna client and setup call | PalmServiceBridge client for `getState`/`setPreferences`/`setEnabled`/`uninstall`, Homebrew `exec setup.sh`, 30 s timeout, resubscribe, visibility pause; replace `TvHostClient` | `frontend/packages/webos-platform` | M | dev host | W7 | P1: refines `prysm-3jx.21` |
| W7 | Export Luna State and Preferences schemas | Schema derive and export for `State`, `Preferences`, `SlimCapture`; generated TS; add `pid`/`revision` or rely on LS2 ordering | `hosts/webos-service`, `tools/export-contracts` | S | dev host | — | P1: refines `prysm-3jx.21` |
| W8 | Rebuild TV UI screens and status logic | Status card states, primary action, settings rows, dialogs, diagnostics, remove-setup confirm, exit confirm, portal address as text/QR, friendly failure text | `frontend/packages/tv-ui` | L | dev host | W6 | P1: refines `prysm-3jx.21` |
| W9 | Port remote navigation, drafts and UI tests | Spatial focus map, key codes 37–40/461/27, hover focus, caret-safe inputs, drafts across updates, the 47 behaviour tests | `frontend/packages/tv-ui`, `frontend/tests` | M | dev host | W8 | P1: refines `prysm-3jx.21` |
| W10 | Make the slim TV UI drive the remote portal | HTTP client for the headless portal from `Origin: null`; instance editor; derive `receiver` from remote `capture.listen`; slim capture size/FPS/menus rows; lights on/off | `frontend/hosts/webos-slim`, `web-portal` access | M | dev host | W6, W11 | P1: refines `prysm-3jx.21` |
| W11 | Restrict portal access to the TV app origin | `Access` option for exactly `Origin: null` and a Host allowlist for the full TV and headless | `surfaces/web-portal`, `hosts/webos` | S | dev host | — | P2: closes the CSRF/DNS-rebinding hole; see Q1 |
| W12 | Accept hostnames and verify the slim receiver | Resolve `host:port` on the TV per connect; a hello/ack so `setPreferences` can be checked like `--verify-instance` | `capture-remote`, `webos-link` args, `webos-service` preferences | M | dev host | — | P2: `prysm.local` setups work today. Done on the dev host (`prysm-3jx.26`): `host:port` resolved per connect; no verify step, the status is the feedback |
| W13 | Forward native NV12 from the slim TV | `PixelFormat::NV12` in `prysm-core`, wire code, receiver conversion; skip on-TV RGB | `prysm-core`, `capture-remote`, `capture-webos` | M | dev host, TV | — | P2: halves bandwidth; refines `prysm-3jx.12`. Done on the dev host (`prysm-3jx.12.1`): wire version 2 with a hello, per-stream encoding (H.264 reserved) and NV12 frames; NV12 sampled in place, no receiver conversion. TV run open (W18). |
| W14 | Re-advertise portal URL after address change | Recompute LAN address on heartbeat; publish the new `portal` | `hosts/webos/src/link.rs` | S | dev host | — | P3: DHCP renewals leave a stale URL |
| W15 | Complete the portal webOS capture panel | Width, height, FPS 1–60, include UI; generated type from the `webos` schema | `frontend/packages/capture-webos-ui`, `tools/export-contracts` | S | dev host | — | P1: full users cannot change resolution |
| W16 | Expose capture rate in TV state | Frames/s and drops in `WorkerStatus` → `State`; optional Prometheus later | `webos-link`, both workers, supervisor | S | dev host | — | P3: replaces metrics for diagnosis |
| W17 | Detect the old package during setup | Refuse or warn when `io.prysm.capture.*` manifests or `init.d/prysm-capture` exist; doc the order (old cleanup, then install) | `hosts/webos-service/src/install.rs`, user docs | S | dev host | — | P1: two boot hooks race for the capture lock |
| W18 | Run TV acceptance for both packages | Plan §10 TV list, plus full 640×360 and 720p FPS/CPU (90 s soak), slim bandwidth at 320×180 and 640×360, wake with Freebox/CEC, cold boot, hung-capture KILL, uninstall | `research/webos-capture` evidence | M | TV | W1, W3, W5 | P1: refines `prysm-3jx.22` |
| W19 | Calibrate from the TV app | Portal calibration API, `calibration-tv.html` relay, pattern clips and MSE player, QR pairing, for both packages | portal + `tv-ui` | L | dev host, TV | W8 | P1: refines `prysm-3jx.11` |
| W20 | Rewrite TV docs for v1 | `docs/users/webos.md`, `docs/developers/{webos,packaging}.md`; delete `hardware-h264.md` | `docs/` | S | dev host | W3 | P1: required at deletion |
| W21 | Decide compression and wanted-frames for slim | LZ4 or H.264 as a new wire-v2 stream encoding; pause frames while the server's lights are off; reconnect without restarting the process | `capture-remote`, `hosts/webos-slim`, headless receiver | M | dev host, TV | W13 | P2: refines `prysm-3jx.12`; see Q2, Q3. Done on the dev host (`prysm-3jx.12.2`): H.264 encodings, pause/resume and keyframe requests; the uplink already reconnects. Hardware encoder and TV run open (W18) |

## 5. Performance and bandwidth facts

| Fact | Value | Source |
| --- | --- | --- |
| On-TV pipeline, serial, 640×360 | 25.3 FPS, 39.5 ms work/frame | [local README](../../apps/prysm-webos-capture/local/README.md) |
| Same with Rayon projection | 30.0 FPS, 1.32 cores | same |
| 1280×720 serial, portable NV12 | 10.65–16.95 FPS; NV12→RGB 11.1 ms | same |
| 1280×720 NEON + parallel grid | 29.97 FPS, NV12→RGB 2.94 ms, 2.02 cores | same |
| Hard-float flags matter | 720p: 5 frames in 25.7 s without them | same |
| Raw RGB24 bandwidth | 320×180@30 ≈ 41.5 Mbit/s; 640×360@30 ≈ 166 Mbit/s | [capture-remote](../../v1/adapters/capture-remote/README.md), arithmetic |
| NV12 is half of RGB24 | 640×360@30 ≈ 83 Mbit/s | arithmetic |
| LZ4 at 640×360@30 sometimes hit the write deadline | — | [deployment history](../../research/webos-capture/deployment-history.md) |
| Hardware H.264 2 Mbit/s target, 640×280@30, 24.8 % of one core | output wait 11.9 ms | [qualification](../../research/webos-capture/hardware-h264-qualification.md) |
| WS adds no latency over TCP; proxied WSS adds ~35 ms p95 | 23 ms capture→DDP p95 | [transport research](../../research/webos-capture/2026-11-04-websocket-transport/README.md) |
| Wake: status 1 for 10 s then process restart recovered video | in-process re-init fixed it (0.1.9) | [readiness](../../research/webos-capture/2026-11-01-readiness/README.md) |
| Boot hooks run with a restricted `PATH` | absolute tool paths required | same |
| Failed `vtCapture_create` leaves stale native state | exit the process, never retry in-process | [lifecycle report](../../research/webos-capture/2026-11-01-lifecycle/report/2026-11-01_reverse-vt-lifecycle-report.md) |
| Capture tap is limited BT.709 NV12, downstream of HDR tone mapping | — | [colorimetry](../../research/webos-capture/2026-11-01-colorimetry/README.md) |
| Worker RSS | ~2 MiB idle, ~6.4 MiB at 320×180/15 H.264 | deployment history |

The TV's network link speed is not recorded. If it is 100 Mbit/s Ethernet or
Wi-Fi, raw 640×360 RGB does not fit (Q2).

## 6. Deletion map

| Root code | Can go once |
| --- | --- |
| `apps/prysm-webos-service` (supervisor, Luna FFI, codegen, files, migrations) | W3, W5, W17, W18 |
| `apps/prysm-webos-capture/src/{capture,native,lock,shutdown,control,events}.rs`, `native/` C++ | W18 (capture verified on the TV) |
| `apps/prysm-webos-capture/src/{transport,session,encoder*,pixels}.rs`, `examples/loopback-receiver.rs`, OpenH264/GStreamer licences | Q2/Q3 answered; W21 if a codec is kept |
| `apps/prysm-webos-capture/local` (`prysm-webos-local`, `portal.rs`, `nv12.*`) | W1, W18 |
| `crates/capture/prysm-webos-protocol` | the two rows above, and the root server's webOS input (desktop audit) |
| `crates/web/prysm-webos` (TV UI, calibration clips, tests) | W6–W10, W19 |
| `packaging/webos/*`, xtask `package webos`, `Contract::Webos`, CI `webos-*` and Miri jobs | W2–W5 |
| `docs/users/webos.md`, `docs/developers/{webos,hardware-h264}.md` webOS parts | W20 |
| `research/webos-capture` | never: evidence, links must keep resolving |

## 7. Open questions

1. **Portal origin policy.** Keep `AnyOrigin` on the full TV and headless, or
   accept only `Origin: null` plus a Host allowlist? Recommendation: add the
   `null`-only option now (W11). It keeps "no token" and closes cross-site
   control from any page a LAN user visits.
   **Decided (`prysm-3jx.56`):** `--allow-origin null` accepts exactly
   `Origin: null`; the slim setup starts the desktop Prysm with it. No relay
   page.
2. **Slim bandwidth.** Raw RGB at the old recommended 640×360 is ~166 Mbit/s.
   Recommendation: send NV12 (W13) and default slim to 320×180. Reintroduce LZ4
   (cheap, lossless) before H.264. Keep hardware H.264 out of v1 unless a
   measurement shows Wi-Fi TVs need it.
   **Decided 2026-11-12:** the hardware H.264 encoder becomes the default for
   the slim stream (`prysm-3jx.12.2`). The recommendation above no longer
   applies. Later the same day: no automatic fallback; `capture.encoding` is
   `h264_hardware`, `h264_software` or `raw`, and an encoder that cannot start
   or fails is a reported worker failure. **Changed by `prysm-3jx.63`:** the
   desktop portal's `remote` capture settings own size, rate, menus and
   encoding, sent to the TV in the wire-version-3 handshake; the TV has no
   capture preference. An encoder failure no longer ends the worker: it is
   reported and the request is refused until it changes.
3. **Only capture when wanted.** Streaming continuously keeps the LG tap and
   ~40 Mbit/s busy while the lights are off. Recommendation: a one-byte
   "pause/resume" from receiver to sender on the existing TCP connection (W21),
   after first ship. **Done in `prysm-3jx.12.2`** for encoding and sending;
   stopping the TV capture itself would need a `Forwarder` hook.
4. **Upgrade from `io.prysm.capture`.** No migration was decided, but both
   packages' boot hooks would run. Recommendation: `setup` refuses while the old
   hook or manifests exist and tells the user to run the old app's **Remove
   setup** first (W17).
5. **Slim receiver addressing.** IP-only `receiver` breaks `prysm.local` and
   https setups. Recommendation: accept `host:port` and resolve on each connect
   (W12); keep TLS out.
6. **Receiver contention.** v1 lets a second TV take over the receiver silently.
   Recommendation: accept for one-TV homes; report "replaced by another sender"
   in the headless snapshot so the UI can explain it.
7. **Docs inconsistency.** [UI-ASSEMBLIES.md](../../v1/UI-ASSEMBLIES.md) still
   describes a Luna bridge surface inside the workers, but the supervisor plan
   gives Luna to the supervisor. Recommendation: fix it with W6.
