# webOS lifecycle and permissions

The TV package contains a web app, a native Luna supervisor and the webOS capturer.
The supervisor owns configuration, power observation and worker recovery. The
webOS capturer runs as a supervised worker, captures video and sends it to a separate Prysm host. For installation,
use [TV setup](../users/webos.md); for ABI/IPC detail, read the
[worker](../../apps/prysm-webos-capture/README.md) and
[supervisor](../../apps/prysm-webos-service/README.md) contracts.

## Capture and power lifecycle

The supervisor runs one worker only while enabled and TV power is confirmed
`Active`. Power-subscription failures keep it stopped and retry after five seconds.
Manual Stop survives standby/wake; cold boot uses the saved autostart choice.
TV autostart never restores Prysm's lighting power: the host starts idle.

The worker connects to `/api/capture` on Prysm's HTTP listener. The first ready TV
gets capture; others receive busy and retry. Idle connections open no capture
devices or encoder. Prysm renews idle replies every second; a complete reply has
a four-second deadline. Screen sync supplies resolution/FPS and starts capture.
Off/effects ends the session. A successful session exit restarts a fresh worker
after one second; unexpected exits retry after five seconds.

A Luna videooutput subscription supplies MAIN-sink readiness. Both video-only and
combined capture wait for connected video with nonzero dimensions/FPS. Video mute
and valid black frames do not count as signal loss. Context changes reset the video
client; duplicate notifications do not. Rapid changes stay latched until consumed.
Unknown readiness allows bounded native retry; failed subscriptions resubscribe.
A frame gap of five seconds ends host capture, allowing a fresh session when video returns.

Native open, preparation and release have a five-second watchdog; preparation
cannot remain unready beyond ten seconds. Standby or Stop sends SIGTERM, followed
by SIGKILL after five seconds if native cleanup blocks. Process isolation and a
hardware lock prevent a stuck driver or duplicate worker from taking over another
session. The supervisor itself recovers through Luna activation or cold boot.

Worker configuration is typed JSON in `PRYSM_WORKER_CONFIG`. Private readiness
commands use stdin; lifecycle events use the inherited `PRYSM_EVENTS_FD` pipe,
separate from vendor stdout/stderr. Both channels are required. Keep the host,
worker and supervisor from the same checkout when changing their contracts.

The TV UI reports waiting for Prysm, ready to capture, waiting for video or
capturing. Capturing describes a request, not proof of fresh frames. Status
subscriptions reconnect without discarding edits or focus. Host startup still
needs processed frames and observed WLED realtime within 12 seconds; otherwise
Screen sync fails and compensates. Wake the TV first.

## Luna permissions and persistent files

Homebrew Channel's fixed `setup.sh` runs `prysm-webos-service --setup`. It validates
owned registrations, installs roles, bounds the Luna rescan and activates `/status`
with a validated reply. Setup does not change saved settings.

| Path | Purpose |
| --- | --- |
| Packaged `supervisor.role.json` and `io.prysm.capture.supervisor.native.json` | Supervisor identity and outbound power/videooutput permissions. Retained so setup/status remain available. |
| Packaged `capture.role.json` and `/var/luna-service2/manifests.d/io.prysm.capture.native.json` | Worker identity and capture/media permissions, separate from the supervisor. |
| `/var/lib/webosbrew/init.d/prysm-capture` | Restore registration and activate the supervisor at boot; log to `/tmp/prysm-capture-setup.log`. |
| `/var/lib/prysm-capture/config.json` | Validated TV settings, atomically saved in a root-only directory. |

The app declares public Luna access; restricted capture and power access still
require the executable roles. The declaration does not establish compatibility
with another TV model, firmware or the LG store.

Only the setup app's Luna identity may mutate service settings. Validate callers
before parsing requests. Never pass receiver settings through a shell. Setup and
cleanup refuse unrelated content at owned manifest/hook paths; LG system manifests
are not modified.

Before uninstalling, use **Service options → Remove service setup… → Remove setup**.
It stops the worker, disables autostart, removes Prysm's capture registration and
rescans Luna. Saved settings remain. Removing the IPK without this step can leave
a guarded boot hook and stale registration; the package has no removal script.
For permission errors, check that Luna loaded the separate capture manifest.

## TV app interaction

Arrows move focus; OK opens an editor or stages a choice; Save applies edits.
Back cancels/leaves a view or confirms closing. Preserve text cursor movement,
visible focus, large targets, pointer input and drafts across status updates.
Closing the UI does not stop the webOS capturer. Setup repair and diagnostics belong in
Service options; removal defaults to retaining setup. See [frontend development](frontend.md#webos-tv-app).
