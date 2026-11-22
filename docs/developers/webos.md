# LG TV: processes, lifecycle and files

Installation for users: [LG TV](../users/webos.md). Building packages:
[packaging](packaging.md#webos). Why two packages and a supervisor:
[decision](decisions/2026-11-12-two-tv-packages.md).

> **Not yet verified on hardware.** The supervisor, both workers and setup are
> tested on the development host and, for the TV crates, under qemu. The only
> recorded run on a TV is the processor's benchmark
> ([sampler speed](../../research/processing/2026-11-15-sampler-speed/README.md)).
> No TV acceptance (install, capture, standby and wake, cold boot, hung
> capture, removal) or on-TV smoke test is recorded for either package
> ([records](../../research/tv-acceptance/README.md)).

> **Safety.** Work only with Prysm's own files: the app and service
> directories, its Luna manifests, its boot hook and `/var/lib/io.github.ericschaal.prysm`.
> Never overwrite the TV's system partitions (kernel, rootfs, tvservice) or
> LG's manifests. Builds do not deploy.

## Processes

A hung LG capture call can only be recovered by killing the process that made
it. Capture therefore runs in a worker process under a supervisor.

| Installed as | Full package | Slim package | Owns |
| --- | --- | --- | --- |
| `prysm-webos-service` (Luna-activated, root) | bin `prysm-webos-service` | bin `prysm-webos-slim-service` | Luna name `io.github.ericschaal.prysm.service`, TV power and video, preferences, the intent, worker start/stop/kill, setup and uninstall |
| `prysm-webos-worker` (child) | `prysm-webos`: engine, webOS capture, WLED, portal on `0.0.0.0:8787` | `prysm-webos-slim`: webOS capture, H.264 or raw stream to a desktop Prysm | Capture and everything after it |

The flavour is compiled into each supervisor binary, and each package ships
only its own pair, so a supervisor cannot start the other package's worker.
Code: [webos-service](../../crates/hosts/webos-service/src/lib.rs),
[webos](../../crates/hosts/webos/src/main.rs),
[webos-slim](../../crates/hosts/webos-slim/src/main.rs),
[webos-link](../../crates/engine/webos-link/src/lib.rs).

## Supervisor and worker

- The worker gets arguments only (no shell): the full worker its settings
  path, listen address and lighting target; the slim worker its Prysm address
  (`--prysm`).
- Commands go to the worker's stdin; events come back on its original stdout,
  one JSON object per line (at most 4 KiB, unknown fields rejected). Vendor
  output is redirected to stderr, which carries logs. The commands are
  `video` (unavailable unless the TV is active), `reset_video` (input, app or
  picture mode changed) and `lighting` (full only; a slim worker takes it as
  protocol misuse); the events `status`, `failed` and `faulted`. The worker
  exits 0 when stopped (SIGTERM, stdin closed), 1 failed, 2 for bad
  arguments, 3 faulted (a hung or unconfirmed native resource). Types:
  [webos-link](../../crates/engine/webos-link/src/lib.rs).
- The worker reports status on every change and every 5 s; 15 s of silence is
  a hang. Stop sends TERM, then KILL 8 s later; a worker still not reaped 8 s
  after its KILL is let go and taken as killed, so a stop always ends.
- An exit is handled once the worker's last events are read, at most 1 s
  after it is reaped (a process it left behind can hold the pipe).
- An exit while running is published as a failure: the exit's description,
  then the last cause or report; an `encoder`, `version` or `replaced`
  report keeps its kind, anything else is `other`.
- Turning it off (`setEnabled` false), the TV going to standby, a shutdown
  and a new Prysm address clear the published failure and the backoff, in
  every phase (also with no worker, during a backoff or while a failed
  worker stops); turning it off is how a failure is dismissed. A stop asked
  for this way still ends in a failure when the worker faults (exit 3, its
  last cause kept), dies of a signal other than SIGTERM, or has to be
  killed: then it may not have released the TV capture. (SIGTERM before
  the worker's handlers are in is the asked-for stop.) While a failed
  worker stops, the supervisor's reason is the published failure. A worker
  let go after its KILL is followed by a backoff, not an immediate start.
- Each status names its package (`"flavor": "full"` or `"slim"`) and has only
  that package's fields; a status of the other package breaks the link. A
  worker whose run ends with an error says why (`failed`, also before its
  first status), then exits (failed, or faulted when the error is a fault).
  A protocol misuse or a lost link exits failed without it.
- The worker stops when its supervisor dies: it gets SIGTERM (parent-death
  signal) and its stdin closes, and it stops as usual, releasing the capture
  tap. Nothing SIGKILLs it then: a worker killed inside a native capture call
  can leave the TV's capture locked until a reboot. One that does not finish
  is bounded by its own stop deadline and, until it exits, holds the capture
  lock `/tmp/prysm-capture.lock`, so no other worker opens a tap meanwhile
  ([`guard_parent`](../../crates/engine/webos-link/src/worker.rs)).
- The supervisor logs which signal stopped it. The worker logs opening and
  releasing the TV UI tap with the capture thread's kernel id (`thread`), the
  id a kernel log names as a capture lock's holder (`K_pid`).
- Without video the capture takes the TV UI. A missing `libhalgal` (or one
  of its symbols) or `/dev/gfx` turns that off until the worker restarts;
  any other UI failure is retried after 5, 10, 20, 40 and 60 s (a frame
  resets the wait), and one after that turns it off too: a UI tap can fail
  for good (a worker that died with it open left the graphics driver's
  lock held until a reboot). A transient UI failure, which the retries are
  for, has not been observed. The first failure of a run logs a warning, the rest debug
  ([native loop](../../crates/adapters/capture-webos/src/source/native.rs)).
- Before each video tap opens, the worker registers its two capture client
  names with Luna and removes them again. Any failure of that check ends
  the capture source at once, without reopening its tap; the cause says
  whether a name was denied ("Invalid permissions"), stayed registered, or
  Luna failed otherwise (LS2 has no typed denial, so that may be a denial
  worded differently). What follows is the package's: the full worker
  re-applies the lighting with its backoff (1 s doubling to 30 s), which
  opens a new source and checks again in the same process; the slim worker
  fails, and the supervisor starts a new one after its backoff. A name that
  stays registered, or a failed driver creation (`vtCapture_create`), cannot
  succeed again in the process, so the full worker fails then too (exit 1)
  instead of retrying in place. Other video failures reopen the tap; the
  first of a run logs a warning and the rest debug. After a failed driver call a tap
  accepts nothing but its release
  ([capture-webos-sys](../../crates/adapters/capture-webos-sys/src/lib.rs),
  [native loop](../../crates/adapters/capture-webos/src/source/native.rs)).
- The supervisor reaps a worker before starting another (or lets go of one
  still not reaped 8 s after its KILL). Failures back off
  from 1 s doubling to 30 s; 60 s of running resets the backoff.
- A worker runs while the TV is on (active or screen saver) and, for slim,
  while capture is enabled and `instance` is set. Video is reported
  unavailable unless the TV is active.
- The intent (full: lighting target; slim: enabled) is kept with the boot ID
  in `resume.json`: a service restart in the same boot resumes it, a reboot
  starts from **Start with TV** (`autostart`).

Full worker: re-applies a non-off target after a session failure (same
backoff), advertises its LAN portal URL (looked up again at every heartbeat),
and seeds `settings.json` with `capture.kind = "webos"` when missing.

Slim worker: connects to the receiver at most once a second, also while
Prysm speaks another capture stream version, resolving a host name at each
attempt; follows the receiver's stream request (size, rate,
menus, encoding); closes capture while the receiver has stopped the stream
(lighting off). An encoder failure is reported with `failure.kind` `encoder`
and refused until the desktop asks for another encoding; nothing falls back on
its own. A receiver whose source failed with the stream (it refused the
request, a keyframe did not decode, or it has no decoder; close code 4002) is
reported with `failure.kind` `receiver_failed`. When another TV takes the
stream over, the worker releases capture
and encoder, reports `failure.kind` `replaced` and connects no more until the
worker restarts (capture stopped and started, the TV waking, or a new Prysm
address), so two TVs never take the stream from each other in turn. A
stopped stream, an encoder failure, a failed receiver or another version
stays the news while the receiver is unreachable. In all four the capture is
closed, and connection attempts go on, so a receiver that asks again
(lighting on, an update, another encoding) reopens it. Its status
carries the uplink as one `link` tagged by `state`: `starting`, `sending`
(`encoding`, `fps`), `paused`, `waiting` (the receiver wait) or `failed`
(the encoder, version, replaced or receiver-failed failure); the supervisor
counts it as up once `sending` or `paused`. A stop shuts the uplink's socket
down, which ends a connect, read or write at once, and no capture opens after
it; what is left is one pump round (its 50 ms pull, and a send's 1 s
hardware encoder wait or 0.5 s host name lookup), the encoder's 0.5 s
end-of-stream wait when it closes, and one capture release (4 s): 5.55 s,
within the slim worker's 6 s stop deadline and before the supervisor's 8 s
KILL. Not bounded, and so not counted: the hardware encoder's PLAYING
transition when the round opens it, and its NULL transition when it closes.
All rules are one transition function
([`Report::next`](../../crates/hosts/webos-slim/src/link.rs)). Wire
protocol: [capture-remote](../../crates/adapters/capture-remote/README.md).

## Supervisor state machine

Every rule above is one sans-IO machine,
[`Machine::handle(Input, now) -> Vec<Effect>`](../../crates/hosts/webos-service/src/machine.rs):
inputs are TV power and video, preferences (already validated and
persisted), enable, worker events, logs and exits, a broken event stream, a
failed spawn, a tick and shutdown; effects are `Spawn`, `Send`, `Terminate`,
`Kill`, `Abandon` (let go of a worker not reaped after its kill) and
`Publish`. Its phases are idle, running, stopping and retrying. A worker is
spawned only from idle, and only a reaped (or let-go) exit leaves running or
stopping, so a worker is always reaped before the next one starts. Time is an
argument, so its tests are tables without sleeps.

- A worker event line that does not decode, is too long or is cut off breaks
  the link: the supervisor stops the worker as failed.
- After the TV app turns the full worker's lighting on or off, worker
  reports older than 2 s (`ECHO`) do not change the published target back.
- While a failed worker waits to restart, the state says the whole seconds
  left, rounded up, as of when it was sent; the app counts down from when it
  received it, so no clock is shared.
- The slim worker's forwarder reports an unconfirmed capture release as a
  failure (exit 1), not a fault; either way the process is restarted.

The driver ([service.rs](../../crates/hosts/webos-service/src/service.rs))
runs it on the GLib thread: requests and subscription replies arrive as
callbacks, and a 100 ms timer drains worker output, reaps, ticks the machine
and renews a subscription that has had no good reply for 5 s. Its seams:

| Seam | On the TV | In tests |
| --- | --- | --- |
| `Bus` | [luna.rs](../../crates/hosts/webos-service/src/luna.rs) over `prysm-webos-luna-sys` | a recording bus |
| `Launcher`, `Worker` | `ProcessLauncher`: pipes, reader threads, signals, reaping | scripted fakes; `sh` scripts as workers |
| `PreferencesStore` | `FileStore` | `MemoryStore` (feature `testkit`) |
| `Installer` | `FsInstaller`: manifests, boot hook, bounded `ls-control` and `luna-send` | a temporary root with script stubs |

## Luna API

Service `io.github.ericschaal.prysm.service`: `getState` (any caller, subscribable),
`setPreferences`, `setEnabled` and `uninstall` (the app only, checked by LS2
application ID). The state has the fields both packages share (`preferences`,
`power`, `video`, `failure`, `log`) and the installed package's own, tagged
by `flavor`: full `lighting` and a `worker` that reports `capturing`, `fps`
and `portal` while it runs (`portal` also while it stops); slim `enabled` and
a `worker` that reports `capturing` and its `link` (as in the worker's
status) while it runs. API version 3 (the document's `info.version`). Preferences hold only `autostart` and, for slim, `instance`
(the desktop Prysm's portal address, where frames go). An absent value (no
failure, `fps`, `portal` or `instance`) is omitted, never `null`,
here and in the worker's status lines. Picture settings live in a
portal, never in Luna. Types: generated `@prysm/contracts/luna` from
[schema.rs](../../crates/hosts/webos-service/src/schema.rs).

The TV app sets the service up through the Homebrew Channel's `exec` of
`setup.sh`, because the service does not exist yet at that point.

## Files on the TV

| Path | Purpose |
| --- | --- |
| `/media/developer/apps/usr/palm/applications/io.github.ericschaal.prysm/` | TV app |
| `/media/developer/apps/usr/palm/services/io.github.ericschaal.prysm.service/` | Supervisor, worker, `setup.sh`, Luna role/service files, licences |
| `/var/luna-service2/manifests.d/io.github.ericschaal.prysm.supervisor.json`, `io.github.ericschaal.prysm.worker.json` | Luna role registration, written by `setup` |
| `/var/lib/webosbrew/init.d/io.github.ericschaal.prysm` | Boot hook; runs `setup` at boot |
| `/var/lib/io.github.ericschaal.prysm/tv.json` | Preferences (supervisor) |
| `/var/lib/io.github.ericschaal.prysm/resume.json` | Intent and boot ID (supervisor) |
| `/var/lib/io.github.ericschaal.prysm/settings.json` | Full only: portal settings (worker's engine) |

`prysm-webos-service setup` writes the manifests and boot hook, rescans Luna
(also after a failed step) and activates the service, all with bounded
waits. A manifest with Prysm's id and role files, and a hook with its
`# Prysm (<app id>):` line, are Prysm's from any release and are replaced
atomically; anything else is refused, and a manifest that is not JSON must
be deleted before setup runs again. It must run after every install or update: IPKs carry no install
scripts. The Luna method `uninstall` (only the app may call it) replies at
once, stops the worker gracefully (TERM, KILL 8 s later), then removes the
hook and manifests, rescans and exits. The `uninstall` subcommand only
deregisters (hook, manifests, rescan); it does not stop a running Prysm.
Preferences, settings and the intent in `resume.json` stay either way.
Remove the setup before the app, or the registration stays behind; removing
the app ends a running Prysm abruptly, so stop it first (the Luna method
does). Installing over a running Prysm ends it abruptly too, so
`install-ipk.sh` and the acceptance runner first call the Luna `uninstall`,
wait until no Prysm process is left (refusing to install while one remains),
then install and run `setup.sh`, which re-registers and resumes the intent.
Installs through Homebrew Channel or the TV's own UI cannot be hooked this
way: an IPK carries no scripts.

Nothing detects or migrates the old `io.prysm.capture` app; it must be removed
first.

## TV app

Arrows move focus between two columns and within rows, OK chooses, Back closes
one dialog level and then asks before closing. Closing the app leaves the
service running. Source: [TV UI](../../frontend/packages/tv-ui/app.tsx);
local demo: [frontend](frontend.md#webos-tv-app).
