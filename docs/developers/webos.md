# webOS lifecycle and permissions

Current package: TV web app, native Rust Luna supervisor, and supervised capture
worker. For local UI development, see the [TV frontend](../../crates/web/prysm-webos/README.md).
For installation, see [TV setup](../users/webos.md).
For ABI/IPC contracts, see the [worker](../../apps/prysm-webos-capture/README.md)
and [supervisor](../../apps/prysm-webos-service/README.md).

## Capture and power lifecycle

The supervisor starts one Rust sender only while enabled and TV power is
`Active`, using `com.webos.service.tvpower/power/getPowerState`. It waits for a
valid power observation rather than assuming an awake TV. Failed subscriptions
retry after five seconds and keep the sender stopped.

Prysm owns the TCP listener while the application is running with webOS input,
including when the pipeline is off. An idle TV connects without opening capture
devices or encoding/sending frames. Screen sync sends capture settings to that
connection; Off or Effects closes the capture session. Resolution and FPS remain
owned by Prysm. After a capture session ends, the worker exits and the supervisor
starts a fresh worker after one second. On this TV, UI reinitialization in one
process retained five device handles; process exit lets the OS reclaim vendor
state. A [focused cleanup diagnosis](../../research/webos-capture/2026-11-01-lifecycle/cleanup-diagnosis.md)
subsequently isolated that growth to HAL library unloading and fixed the library
lifetime. Video constructor failures still retain resources, and native video
release has a reproduced lost-wakeup hang, so process isolation remains in place. Each worker handles one connection and exits if connection fails or the receiver
disconnects. The supervisor owns retries.

The TV UI distinguishes **Waiting for Prysm**, **Connected · Capture off**, and
**Capturing**. Capturing means the worker initialized capture and is attempting
to send frames; it does not guarantee fresh HDMI video. Driver errors remain
available in diagnostics. The app subscribes to native status changes. A failed subscription is cancelled
and retried after three seconds; returning to the app refreshes it. Unsaved
settings and focus survive status updates.

The supervisor subscribes to `com.webos.service.videooutput/getStatus` and sends
MAIN source readiness over a private, nonblocking stdin control pipe. A usable
source has a connected MAIN sink and nonzero dimensions/frame rate; it is not
restricted to an HDMI port or app. Video mute is not treated as a capture failure.
Both sources wait without sending frames while video is absent. Native
initialization resumes immediately when readiness returns.
Input context changes reset only the video client. Duplicate notifications do not
reset it; rapid loss/return transitions are retained until the worker consumes them,
including when readiness becomes unknown before delivery.

Native video open, preparation, and release have a five-second watchdog, including
release after acquisition errors. A hung reset, failed constructor,
or preparation that stays unready for ten seconds still triggers process recovery
with the existing bounded teardown and failure backoff. Ordinary video absence
reported by Luna does not start a restart cycle. Failed/malformed subscriptions
fall back to bounded capture retries and resubscribe after five seconds. The worker retains bounded native retry for unknown readiness. Valid black
frames never expire.

The service receives typed JSON events over a dedicated inherited pipe, separate
from vendor stdout and diagnostic stderr. `PRYSM_EVENTS_FD` identifies the private
descriptor. Both the event pipe and stdin control channel are required. The
supervisor launches the worker without arguments and supplies typed receiver,
source, compression, and metrics settings in `PRYSM_WORKER_CONFIG`; commands never enter
the receiver protocol. The supervisor clears connectivity on disconnect
before native teardown, and kills a worker after five seconds if cleanup blocks.
Upgrade native executable and supervisor together using the IPK. The always-on
idle connection also requires rebuilding/restarting the host Prysm application.

On standby the supervisor sends SIGTERM and waits for native cleanup. It sends
SIGKILL after five seconds if the driver blocks. Wake starts a fresh sender if
it remains enabled. Manual Stop stays stopped across standby/wake; at cold boot
the saved autostart choice applies. Unexpected sender exits retry after five
seconds. A process lock prevents multiple updated Prysm capturers from acquiring
the hardware concurrently, including after supervisor failure.

The panel still requires startup confirmation within 12 seconds and rolls back
an unsuccessful Screen sync request. Wake the TV before enabling Screen sync.
Prysm itself still starts idle; TV autostart does not turn on the lights or restore
Prysm's previous lighting intent. A supervisor process crash is recovered by the
next Luna activation or cold boot, rather than a separate watchdog.

## Luna permissions and persistent files

Homebrew Channel runs the fixed packaged `setup.sh`, which executes
`prysm-webos-service --setup`. Rust checks ownership of its existing manifests,
registers the capture role and native supervisor, runs a bounded Luna rescan and
activates `/status`. Activation keeps stdin open for this TV's `luna-send` and
requires a successful JSON reply. Setup does not change saved settings or autostart.

`io.prysm.capture.supervisor.native.json` references the packaged supervisor role,
dynamic service definition, client and API permissions. Its executable can own
only the Prysm service identity, with outbound access to TV power and videooutput readiness. Cleanup
keeps this registration so status and setup remain available.

The app declares `requiredACG: ["public"]`, matching our custom public Luna API
and Homebrew setup bridge. LG's [ACG guide](https://webostv.developer.lge.com/develop/guides/acg-guide)
says webOS 25 does not enforce the new declaration, webOS 26 supports it, and
webOS 27 requires the field for submission. An empty list would misdescribe this
app, which calls Luna APIs. This is a rooted TV application: the declaration does
not grant restricted capture or TV-power access, replace the explicit executable
roles, or establish compatibility with webOS 26/27 or the LG store.

Native capture has a separate, narrower executable role:

- `/media/developer/apps/usr/palm/services/io.prysm.capture.service/capture.role.json`
  names the packaged `prysm-webos-capture` executable and allows capture-client
  and media-resource-manager identities, with outbound access to capture,
  capturepermission, and media services.
- `/var/luna-service2/manifests.d/io.prysm.capture.native.json` registers that role.
  Setup calls `ls-control scan-services`. LG's capture manifest is not changed.
- `/var/lib/webosbrew/init.d/prysm-capture` restores native registration before
  activating the supervisor at boot. The hook backgrounds
  setup, logs to `/tmp/prysm-capture-setup.log`, and checks that the package exists.
- `/var/lib/prysm-capture/config.json` stores validated receiver/layers/codec/
  autostart settings, with a root-only directory and atomic file replacement.
  Settings survive package upgrades and cleanup.

Setup and cleanup refuse to overwrite unrelated content at their manifest/hook
paths. No system manifest, root filesystem overlay, network control port, or
remote shell command supplied by a caller is added. Mutation methods accept
only the setup app's Luna caller identity. Receiver settings are validated and passed to the worker through its typed startup environment, without a shell.

Before uninstalling, open **Service options → Remove service setup… → Remove setup**. This
disables autostart, stops/joins the sender, removes Prysm's native role manifest,
and rescans Luna. The package manager removes the app/service files and their
generated service registration. Removing the IPK first can leave a harmless
guarded boot hook and stale native registration until cleanup/reboot; the IPK
does not have an automatic removal script. Saved receiver settings remain.

If VTCapture reports `LSRegisterPubPriv FAILED` or permission denied, validate
that Luna actually loaded the separate native manifest on this firmware. The package uses its own standalone registration; it does not apply a system
manifest overlay. Historical overlay procedures remain in the research archive.

Historical installations and validation runs are in the
[deployment archive](../../research/webos-capture/deployment-history.md).

## TV app interaction

Arrows move focus without changing settings; OK opens an editor or stages a choice.
Save applies edits explicitly. Back cancels an editor, leaves service subviews, or
asks before closing. Text fields retain cursor movement. Keep visible focus, large
targets and pointer support; avoid native dropdowns that change during navigation.
Status subscriptions must preserve unsaved edits and focus across reconnects.
Setup repair and diagnostics live under Service options. Removing setup requires
confirmation and defaults to retaining it. Closing the app does not disable the sender.
