# LG TV: processes, lifecycle and files

Installation for users: [LG TV](../users/webos.md). Building packages:
[packaging](packaging.md#webos). Why two packages and a supervisor:
[decision](decisions/2026-11-12-two-tv-packages.md).

> The full package has been exercised on a rooted LG TV with WLED. See the
> [hardware acceptance runner](acceptance.md#tv) for the lifecycle checks
> and the [processor benchmark](../../research/processing/2026-11-15-sampler-speed/README.md)
> for sampler measurements.

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
  path and listen address. It reconciles the remembered mode with the lights at startup. The slim worker gets its
  Prysm address (`--prysm`).
- Commands go to the worker's stdin; events come back on its original stdout,
  one JSON object per line (at most 4 KiB, unknown fields rejected). The
  worker's first statement keeps that stdout for events and points fd 1 at
  stderr, which carries logs, so argument parsing, logging and vendor output
  cannot reach the stream; `clippy::print_stdout` is denied in the workspace
  besides. The supervisor keeps the last 20 lines of the worker's stderr and
  of its own log in the state's `log` (the app's **View diagnostics**): the
  TV starts the supervisor with stderr on `/dev/null`, so that tail is where
  its lines show. Its log writer only queues them (at most 20, the oldest
  dropped) and wakes the loop, which adds them to the tail at its next poll,
  before the worker lines it reads then. The commands are
  `video` (`unknown`, `unavailable` or `available`; unavailable unless the
  TV is active), `reset_video` (input, app or picture mode changed),
  `observe_video` (whether the video subscription is live), and `target` (full only, numbered from 1 by
  `sequence`). `resume` requests On in the engine's remembered mode and
  uses the same sequence. `recover_screen_sync` replaces the requested
  Screen sync session after a worker restart. It honors a current WLED
  override. A slim worker treats these commands as protocol misuse. The events
  `status` (the full worker's says the `sequence` it last `applied`),
  `initialized` (the requested operation reached stable operation), and
  `failed`. The worker exits 0 when stopped (SIGTERM, stdin closed) and 1
  otherwise; the supervisor gives it no meaning. Types:
  [webos-link](../../crates/engine/webos-link/src/lib.rs).
- The worker reports status on every change and every 5 s; 15 s of silence is
  a hang. Stop sends TERM, then KILL 8 s later; a worker still not reaped 8 s
  after its KILL is abandoned. No replacement starts until the process
  is confirmed gone.
- An exit is handled once the worker's last events are read, at most 1 s
  after it is reaped (a process it left behind can hold the pipe).
- A failure carries its escalation, copied where the error is met: the
  worker's `failed` event, a full status's `failure`, or the supervisor's
  own observation (a failed spawn, a broken link, 15 s of silence, an exit
  without `failed`, a worker let go after its KILL). The supervisor reads it
  in one place ([`recovery.rs`](../../crates/hosts/webos-service/src/machine/recovery.rs)).
  Typed worker failures take precedence over the generic crash rule. A
  typed `restart` can replace the process once, and the allowance is
  restored when a worker finishes initialization or the intent changes.
  After an unexpected crash or hang, a stable worker is replaced as soon
  as it is reaped while still wanted. A replacement starts in
  initialization, and a crash during initialization surfaces the cause.
  A heartbeat is only liveness. A native tap can initialize before its
  first frame, and Screen sync also requires the requested output to
  reach its running state. An ordinary restart has no cooldown or backoff.
- A choice of the person or the TV (a lighting target, capture on or off,
  standby, a shutdown, a new Prysm address) answers what is asked and gives
  the restart back; a worker that stops then is followed only by a new one
  if one is wanted. While it asks, the full TV app sends the lighting modes
  and Try again to `setTarget`, since no portal answers; after setting the
  service up again, the app answers it the same way (slim: `setEnabled`).
- Each status names its package (`"flavor": "full"` or `"slim"`) and has only
  that package's fields; a status of the other package breaks the link. A
  worker whose run ends with an error says why (`failed`, also before its
  first status), then exits 1. A protocol misuse or a lost link exits 1
  without it.
- The worker stops when its supervisor dies: it gets SIGTERM (parent-death
  signal) and its stdin closes, and it stops as usual, releasing the capture
  tap. Nothing SIGKILLs it then: a worker that died with the TV UI capture
  open was once seen to leave the TV's capture locked until a reboot. Later
  kill tests did not reproduce it (eight kills, each recovered in 1–2 s
  without a TV restart), so it is rare, not excluded. One that does not finish
  is bounded by its own stop deadline and, until it exits, holds the capture
  lock `/tmp/prysm-capture.lock`, so no other worker opens a tap meanwhile
  ([`guard_parent`](../../crates/engine/webos-link/src/worker.rs)).
- The supervisor logs which signal stopped it. The worker logs opening and
  releasing the TV UI tap with the capture thread's kernel id (`thread`), the
  id a kernel log names as a capture lock's holder (`K_pid`).
- Without video the capture takes the TV UI. A missing `libhalgal` (or one
  of its symbols) or `/dev/gfx` turns that off until the next capture
  source opens. Any other UI failure (a failed open, allocation or capture)
  stops the tries until the video signal changes: video comes back and goes
  again, or the input, app or picture mode changes. Never on a timer: a UI
  tap can fail for good (a worker that died with it open once left the graphics
  driver's lock held until a reboot; later kills did not reproduce it). The first failure since a UI frame
  logs a warning, the rest debug. The capture thread waits for a change of the video signal
  without polling: each change wakes it
  ([native loop](../../crates/adapters/capture-webos/src/source/native.rs)).
- Before each video tap opens, the worker registers its two capture client
  names with Luna and removes them again. Any failure of that check ends
  the capture source at once, without reopening its tap; the cause says
  whether a name was denied ("Invalid permissions"), stayed registered, or
  Luna failed otherwise (LS2 has no typed denial, so that may be a denial
  worded differently). A denial asks to set the service up again; a name
  that stays registered, or a failed driver creation (`vtCapture_create`),
  cannot succeed again in the process, so it escalates `restart`; any other
  Luna failure asks to try again. The full worker reports it in its status
  and keeps running; the slim worker fails. Every other video failure has one row in
  the [classification](../../crates/adapters/capture-webos/src/classify.rs).
  A buffer status 12 (the TV's resource policy took the capture) or a plane
  status 13 (no video plane, as under AirPlay mirroring) releases the tap,
  makes pulls `Pull::Refused` and waits for the next change of the video
  signal, which opens a new tap (not yet verified on hardware). An ordinary
  native fault can release and reopen the tap once. A stall (no buffer
  for 2 s) also reopens the tap once. A frame or video-signal change
  resets stall recovery. If the replacement stalls, release the tap and
  end the session with a capture-stall cause. A live video subscription
  makes capture Waiting until the signal changes. Without a live
  subscription, capture is Blocked. After a failed
  driver call a tap accepts nothing but its release
  ([capture-webos-sys](../../crates/adapters/capture-webos-sys/src/lib.rs),
  [native loop](../../crates/adapters/capture-webos/src/source/native.rs)).
- The TV can refuse to capture the video it shows: `vtCapture_init`
  answers 17. Observed on one TV (webOS 10.3.2): AirPlay video was refused
  on every attempt in two sessions; one YouTube video start in about
  fifteen was refused and stayed refused until the next video. The cause is
  not known. The TV's capture-permission service
  (`com.webos.service.capturepermission`) refuses Prysm's supervisor: it is
  reserved to the TV's own capture services.
- After a 17 the tap is kept with its driver created and not
  initialized: no timer runs, no preparation limit, and pulls are
  `Pull::Refused`. A refused capture is initialized again only when the
  video output's signal changes (availability, or input, app or picture
  mode); refused again, it waits again. Whether a driver is only created
  is what the driver reached (`VtCapture::created_only`), so one that
  answered not ready (1) is kept the same way: a change of context initializes it again instead
  of releasing it, since a created driver holds no video context (not yet
  verified on hardware after a change of app). Its release can hang in the
  vendor's destructor
  ([cleanup diagnosis](../../research/webos-capture/2026-11-01-lifecycle/cleanup-diagnosis.md#video-release-has-a-reproduced-lost-wakeup-race)),
  so it is released only when video becomes unavailable (the tap kind
  changes), when capture stops (also when the preparation limit ends the
  source), and after a native failure of the initialization, which leaves
  the driver accepting only its release; each can still hang and fault the
  worker.
- Within a video, pulls are refused from the first refusal (17) or
  withholding (12 or 13) until a frame, until video becomes
  unavailable, or until the source fails; nothing else ends it, not a
  not-ready or pending answer, nor a native failure that opens a new
  driver.
- The engine reports refused pulls as `input.picture` `refused`. The full
  TV app reads that from its portal, the slim worker reports its connected
  `link` as `refused`; the TV app then shows **Video can’t be captured**
  with no Try again (slim keeps its Stop capture), its diagnostics and the
  portal's light card say "The TV doesn’t let Prysm capture this video."
  Neither is a failure, and both clear once frames come again. A slim
  stream carries no reason, so its receiver's portal only waits for the
  picture. The capture thread logs at info the first refusal or
  withholding since a frame (a refusal with the milliseconds since the last
  signal change), when a signal change initializes the refused driver
  again, and the frame that ends the refused pulls (with their
  milliseconds); repeats, and refused pulls ended by video gone, are debug.
  Each initialization's answer, and each signal change, is debug, except
  one info line per change of `video`. A tap's open only creates the
  driver; its first acquisition initializes it, so a refusal's count of
  initializations in a row includes every initialization.
- The supervisor reaps a worker before starting another (or lets go of one
  still not reaped 8 s after its KILL, which asks to restart the TV: it may
  still hold the capture). Only that worker's late reap, or the TV's
  restart, ends the ask; a choice made meanwhile applies after it.
- A worker runs while the TV is on (active or screen saver) and, for slim,
  while capture is enabled and `prysm_address` is set. Video is reported
  unavailable unless the TV is active.
- The service keeps its last displayed full target or slim enablement in
  `/run/io.github.ericschaal.prysm-resume.json` for the current boot. Slim
  uses **Start with TV** (`autostart`) after a TV reboot. The full worker reconciles
  physical power and display with the mode stored by the engine at every launch.

The full worker waits for known recoverable adapter conditions. A blocked
session stays Off until a new request. The worker reports a capture stall
as a failure only when the session is blocked. While lighting waits for an
unreachable WLED device, the status reports that issue in `lights`
instead. It advertises its
LAN portal URL (looked up again at every heartbeat), and seeds
`settings.json` with `capture.kind = "webos"` when missing.

Slim worker: connects to the receiver at most once a second, also while
Prysm speaks another capture stream version, resolving a host name at each
attempt; follows the receiver's stream request (size, rate,
menus, encoding); closes capture while the receiver has stopped the stream
(lighting off). An encoder failure is reported as the link `encoder_failed`
and refused until the desktop asks for another encoding; nothing falls back on
its own. A receiver whose source failed with the stream (it refused the
request, a keyframe did not decode, or it has no decoder; close code 4002) is
reported as `receiver_failed`. What waiting cannot fix ends the worker
with its cause, and the supervisor asks: another TV taking the stream
over or a reply that breaks the protocol (try again), a wrong address
(404) or an untrusted certificate (change the settings). So two TVs
never take the stream from each other in turn. A
stopped stream, an encoder failure, a failed receiver or another version
stays the news while the receiver is unreachable. In all four the capture is
closed, and connection attempts go on, so a receiver that asks again
(lighting on, an update, another encoding) reopens it. Its status
carries the uplink as one `link` tagged by `state`: `starting`, `sending`
(`encoding`, `fps`), `refused` (connected, but the TV refuses to share the
video it shows, so nothing goes out until the video changes), `paused`,
`waiting` (the receiver wait), `capture_waiting` (a stall waiting for a
video-signal change), `encoder_failed`, `version_differs` or
`receiver_failed`, the last five with their `message`. The slim worker
reports initialization as soon as capture initializes for the receiver's
request. A stop shuts the uplink's socket
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
failed spawn, a tick (at the time `Machine::next_due` names) and shutdown;
effects are `Spawn`, `Send`, `Terminate`, `Kill`, `Abandon` (let go of a
worker not reaped after its kill) and `Publish`. Its phases are idle, running, stopping and asking. A worker is
spawned only from idle, and only a reaped (or let-go) exit leaves running or
stopping, so a worker is always reaped before the next one starts. Time is an
argument, so its tests are tables without sleeps.

- A worker event line that does not decode, is too long or is cut off breaks
  the link: the supervisor stops the worker as failed.
- After the TV app sets the full worker's target, a worker report that has
  not applied that `target` command yet (its `applied` is lower than the
  command's `sequence`) does not change the published target back; a new
  worker counts from 0 again.
- A stop that will ask publishes its `problem` while the worker stops; a
  stop followed by a restart, or by nothing, publishes none.
- An unconfirmed capture release escalates `restart` in both workers: the
  slim worker's forwarder fails with it, the full worker's session faults.

The driver ([service.rs](../../crates/hosts/webos-service/src/service.rs))
runs it on the GLib thread without polling: requests and subscription
replies arrive as callbacks, and a socket wakes the loop when the worker
writes a line or its output ends, a child exits (SIGCHLD) or a stop signal
comes. After each of these, and at the next due time (the machine's
`SILENCE`, kill and letting go times, the end of an
exited worker's 1 s wait for its last lines, a subscription renewal
every 5 s until it is live, and a probe's `--run-for`), the driver drains worker
output, reaps, ticks the machine and renews subscriptions, then arms one
timer for the next due time.

The loop needs only GLib 2.0 functions, all looked up when the library
loads, so a missing one stops the supervisor at its start: a private
context and loop (`g_main_context_new`, `g_main_loop_new`, `_run`, `_quit`,
`_unref`), one-shot timers (`g_timeout_source_new`, `g_source_*`) and the
wake socket's watch (`g_io_channel_unix_new`, `g_io_create_watch`,
`g_io_channel_set_close_on_unref`, `g_io_channel_unref`). The watch is an IO
channel rather than `g_unix_fd_source_new`, which exists only since GLib
2.36, because the repository records the TVs' GLib only for webOS 25
(2.72.3). The channel never closes the socket; its owner does, once.

Its seams:

| Seam | On the TV | In tests |
| --- | --- | --- |
| `Bus` | [luna.rs](../../crates/hosts/webos-service/src/luna.rs) over `prysm-webos-luna-sys` | a recording bus |
| `Launcher`, `Worker` | `ProcessLauncher`: pipes, reader threads, signals, reaping | scripted fakes; `sh` scripts as workers |
| `PreferencesStore` | `FileStore` | `MemoryStore` (feature `testkit`) |
| `Installer` | `FsInstaller`: manifests, boot hook, bounded `ls-control` and `luna-send` | a temporary root with script stubs |

## Luna API

Service `io.github.ericschaal.prysm.service`: `getState` (any caller, subscribable),
`setPreferences`, `setTarget` (full), `setEnabled` (slim) and `uninstall`
(the app only, checked by LS2 application ID). Each package refuses the
other's lighting method. `setTarget { target }` takes `off`, `screen_sync`,
`effects`, or `remembered` and replies with the current state. With
`remembered`, the worker requests On using its persisted lighting mode.
A request made while the worker is unavailable waits for the worker to
start. A later request supersedes any unsent one. `setEnabled { enabled }`
says whether slim capture streams; slim has no target, since the desktop
Prysm decides what the lights do. The state has the fields both packages
share (`preferences`, `power`, `video`, `unreadable`, `log`) and the
installed package's own, tagged by `flavor`: full `target` and a `worker`
that reports `capturing`, `fps`, `lights` (the lights' device issue, such
as `unreachable` while lighting waits for lights that do not answer) and
`portal` while it runs (`portal` also while it stops); slim `enabled` and a
`worker` that reports `capturing` and its `link` (as in the worker's
status) while it runs. The worker's `phase` is `stopped`, `running`,
`stopping` or `asking`. A `problem` (`asked`, `message`) says what the
person is asked: always while asking, while a stop will ask, and (full)
what a running worker asks. `asked` is `{ "step": "unanswered" }` (the
lights do not answer and Prysm stopped trying) or `{ "step": "ask", "fix":
… }` with `try_again`, `change_settings`, `set_up`, `unsupported` or
`restart_tv`. For slim, `autostart` controls streaming on TV boot, and
a service restart within one boot restores enablement from the file in `/run`.
The file contains a JSON string (`"off"`, `"screen_sync"` or `"effects"`).
On the first upgrade from the persistent `resume.json` format, slim uses
**Start with TV**. Later service restarts resume the current boot’s choice. API version 5 (the document's `info.version`).

Values the service sends are open, as on HTTP
([decision](decisions/2026-11-22-open-response-enums.md)): the app reads a
newer `target` or worker `phase` as `unknown` (never off or stopped; a phase
shows "Updating…" with no start or stop), a newer `asked` step or fix as
`try_again`, a newer full `lights` issue in general words ("Not showing screen
sync"), a newer slim `link.state` as starting, a newer `link.encoding` as
sent, and a newer `flavor` as a state it cannot show. Requests stay closed:
the service refuses a value it does not know. Preferences hold only
`autostart` and, for slim, `prysm_address` (the desktop Prysm's portal
address, where frames go). An absent value (no `problem`, `unreadable`,
`fps`, `lights`, `portal` or `prysm_address`) is omitted, never `null`, here and in the worker's status
lines. Picture settings live in a portal, never in Luna. Types: generated
`@prysm/contracts/luna` from
[schema.rs](../../crates/hosts/webos-service/src/schema.rs).

A failed call replies `{ "returnValue": false, "errorKind": …, "errorText": … }`.
`errorKind` is `too_large` (payload over 4096 bytes), `unknown_method`,
`forbidden` (only the TV app may change Prysm), `removing` (the service is
being removed), `invalid_request` (not the method's request),
`wrong_package` (the other package's lighting method),
`invalid_preferences` (a preference the package does not have),
`save_failed` or `internal` (the service could not read the call or send
its reply). It is open: the app reads a kind it does not know, and LS2's own
replies without one, as `unknown`, and shows `errorText` either way. It is
not `errorCode`, which LS2's own replies use for a number.

`tv.json` is shared by both packages (they install as the same app) and
read tolerantly: each package reads the fields it owns, and keeps every
other field as read (full keeps slim's `prysm_address`; a newer version's
fields stay too). `setPreferences` replaces only the installed package's
fields and writes the rest back with them. A file that cannot be read (not a
regular file, too large, not JSON, a field of the wrong type, or for slim an
invalid address) is reported in `unreadable`, the defaults apply, and the
file is left as it is until the next `setPreferences` replaces it. The app
says what is asked first.

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
| `/run/io.github.ericschaal.prysm-resume.json` | Supervisor intent for the current boot |
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
Preferences and settings survive an uninstall. The supervisor intent remains available until reboot.
Remove the setup before the app, or the registration stays behind; removing
the app ends a running Prysm abruptly, so stop it first (the Luna method
does). Installing over a running Prysm ends it abruptly too, so
`install-ipk.sh` and the acceptance runner first call the Luna `uninstall`,
wait until no Prysm process is left (refusing to install while one remains),
then install and run `setup.sh`. The full worker checks WLED at startup; slim
resumes streaming if enabled.
Installs through Homebrew Channel or the TV's own UI cannot be hooked this
way: an IPK carries no scripts.

Nothing detects or migrates the old `io.prysm.capture` app; it must be removed
first.

## TV app

Arrows move focus between two columns and within rows, OK chooses, Back closes
one dialog level and then asks before closing. Closing the app leaves the
service running. The bus has two replies, both observed on a TV with
`errorCode` -1 and no `errorKind`, saying it cannot give the service:
`Service does not exist: <name>.` (never registered) and
`<name> is not running.` with `serviceName` (installed or updated without
setup, and sent to a subscriber when the service ends). On a live
subscription such a reply only ends it: the app keeps the last state and
subscribes again 3 s later, and since the service is dynamic a fresh call
starts it again after setup. A fresh call answered so means the bus cannot
start it: the app counts the service as missing (**Prysm needs setting
up**; a `set_up` ask says the same), as after Remove setup. Any other
failure (a timeout, another error) keeps the last state shown, or
**Connecting…** before any.
Source: [TV UI](../../frontend/packages/tv-ui/app.tsx);
local demo: [frontend](frontend.md#webos-tv-app).
