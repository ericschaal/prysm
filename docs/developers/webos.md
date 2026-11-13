# LG TV: processes, lifecycle and files

Installation for users: [LG TV](../users/webos.md). Building packages:
[packaging](packaging.md#webos). Design and state machine:
[webOS supervisor plan](../plans/prysm-v1-webos-supervisor.md).

> **Not yet verified on hardware.** The supervisor, both workers and setup are
> tested on the development host and, for the TV crates, under qemu. TV
> acceptance (install, capture, standby and wake, cold boot, hung capture,
> removal) is still open for both packages, and the on-TV smoke test has not
> run yet.

> **Safety.** Work only with Prysm's own files: the app and service
> directories, its Luna manifests, its boot hook and `/var/lib/io.prysm.tv`.
> Never overwrite the TV's system partitions (kernel, rootfs, tvservice) or
> LG's manifests. Builds do not deploy.

## Processes

A hung LG capture call can only be recovered by killing the process that made
it. Capture therefore runs in a worker process under a supervisor.

| Installed as | Full package | Slim package | Owns |
| --- | --- | --- | --- |
| `prysm-webos-service` (Luna-activated, root) | bin `prysm-webos-service` | bin `prysm-webos-slim-service` | Luna name `io.prysm.tv.service`, TV power and video, preferences, the intent, worker start/stop/kill, setup and uninstall |
| `prysm-webos-worker` (child) | `prysm-webos`: engine, webOS capture, WLED, portal on `0.0.0.0:8787` | `prysm-webos-slim`: webOS capture, H.264 or raw stream to a desktop Prysm | Capture and everything after it |

The flavour is compiled into each supervisor binary, and each package ships
only its own pair, so a supervisor cannot start the other package's worker.
Code: [webos-service](../../v1/hosts/webos-service/src/lib.rs),
[webos](../../v1/hosts/webos/src/main.rs),
[webos-slim](../../v1/hosts/webos-slim/src/main.rs),
[webos-link](../../v1/crates/webos-link/src/lib.rs).

## Supervisor and worker

- The worker gets arguments only (no shell): the full worker its settings
  path, listen address and lighting target; the slim worker its receiver.
- Commands go to the worker's stdin; events come back on its original stdout,
  one JSON object per line (at most 4 KiB, unknown fields rejected). Vendor
  output is redirected to stderr, which carries logs.
- The worker reports status on every change and every 5 s; 15 s of silence is
  a hang. Stop sends TERM, then KILL 8 s later. The worker dies with its
  parent.
- The supervisor reaps a worker before starting another. Failures back off
  from 1 s doubling to 30 s; 60 s of running resets the backoff.
- A worker runs while the TV is on (active or screen saver) and, for slim,
  while capture is enabled and a receiver is set. Video is reported
  unavailable unless the TV is active.
- The intent (full: lighting target; slim: enabled) is kept with the boot ID
  in `resume.json`: a service restart in the same boot resumes it, a reboot
  starts from **Start with TV** (`autostart`).

Full worker: re-applies a non-off target after a session failure (same
backoff), advertises its LAN portal URL (looked up again at every heartbeat),
and seeds `settings.json` with `capture.kind = "webos"` when missing.

Slim worker: connects to the receiver at most once a second, resolving a host
name at each attempt; follows the receiver's stream request (size, rate,
menus, encoding); closes capture while the receiver has paused the stream. An
encoder failure is reported as `failure_kind: encoder` and refused until the
desktop asks for another encoding; nothing falls back on its own. Wire
protocol: [capture-remote](../../v1/adapters/capture-remote/README.md).

## Luna API

Service `io.prysm.tv.service`: `getState` (any caller, subscribable),
`setPreferences`, `setEnabled` and `uninstall` (the app only, checked by LS2
application ID). Preferences hold only `autostart` and, for slim, `instance`
(the desktop portal) and `receiver` (`host:port`). Picture settings live in a
portal, never in Luna. Types: generated `@prysm/contracts/luna` from
[schema.rs](../../v1/hosts/webos-service/src/schema.rs).

The TV app sets the service up through the Homebrew Channel's `exec` of
`setup.sh`, because the service does not exist yet at that point.

## Files on the TV

| Path | Purpose |
| --- | --- |
| `/media/developer/apps/usr/palm/applications/io.prysm.tv/` | TV app |
| `/media/developer/apps/usr/palm/services/io.prysm.tv.service/` | Supervisor, worker, `setup.sh`, Luna role/service files, licences |
| `/var/luna-service2/manifests.d/io.prysm.tv.supervisor.json`, `io.prysm.tv.worker.json` | Luna role registration, written by `setup` |
| `/var/lib/webosbrew/init.d/io.prysm.tv` | Boot hook; runs `setup` at boot |
| `/var/lib/io.prysm.tv/tv.json` | Preferences (supervisor) |
| `/var/lib/io.prysm.tv/resume.json` | Intent and boot ID (supervisor) |
| `/var/lib/io.prysm.tv/settings.json` | Full only: portal settings (worker's engine) |

`prysm-webos-service setup` writes the manifests and boot hook (refusing
unrelated content), rescans Luna and activates the service, all with bounded
waits. It must run after every install or update: IPKs carry no install
scripts. `uninstall` (Luna method or subcommand) stops the worker, removes the
hook and manifests and rescans; preferences and settings stay. Remove the
setup before the app, or the registration stays behind.

Nothing detects or migrates the old `io.prysm.capture` app; it must be removed
first.

## TV app

Arrows move focus between two columns and within rows, OK chooses, Back closes
one dialog level and then asks before closing. Closing the app leaves the
service running. Source: [TV UI](../../v1/frontend/packages/tv-ui/app.tsx);
local demo: [frontend](frontend.md#webos-tv-app).
