# Prysm Capture for rooted LG TVs

The IPK installs a TV web app (`io.prysm.capture`), Rust Luna supervisor
(`io.prysm.capture.service`) and Rust ARMv7 capture worker. It requires a rooted
TV with Homebrew Channel root execution; boot hooks are needed only for opt-in
autostart. Neither the supervisor nor setup uses Node or `webos-service`.

The tested target is LG webOS 25 (10.3.2-34). Version 0.1.6 was installed on
2026-11-01 over 0.1.5, preserving saved settings, sender state, boot-hook state
and the existing capture-permission mount. The native service runs as root;
the actual TV web app status subscription was verified with 0.1.5.

On Home, persistent video-not-ready status triggered worker exit and restart.
Returning to HDMI1 recovered Freebox video automatically, confirmed on the
receiver by the user, with advancing receiver frames and a stable worker.
Standby/wake and cold boot remain unverified.
Version 0.1.7 adds event-driven readiness and the private worker control channel.
It was deployed with settings and permission mounts preserved; installed HDMI
capture and receiver connectivity were confirmed before transition testing.

## Build and install

Install the [webOS CLI](https://github.com/webos-tools/cli), which supplies
`ares-package`, and the relocated TV SDK described in the
[capturer README](../../crates/capture/webos-capture/README.md). From the repo root:

```sh
npm install --global @webos-tools/cli@3.2.6
WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
  ./packaging/webos/build-ipk.sh
```

The script requires Python 3.9+ and `ar`, builds both binaries in one Cargo invocation, includes license notices, and creates
`target/webos-ipk/io.prysm.capture_0.1.11_all.ipk`. The version comes from
`app/appinfo.json`. The package suffix follows the
webOS app packager convention; both executables are ARMv7 soft-float.
The final archive has root-owned files/directories with no group/other write
permissions. The packaging check rejects install scripts, links, special files,
and payload paths outside the Prysm app/service/package directories. The checker normalizes the CLI's archive ownership and modes before installation.

The CLI may still print its generic Node-version warning whenever any service
is packaged. This IPK contains no Node service files or `package.json`; that
warning does not describe the native runtime.

With a configured ares TV device, install using `ares-install --device TV
target/webos-ipk/io.prysm.capture_0.1.11_all.ipk`. Alternatively copy the IPK to
`/tmp` on the TV and install from a root shell:

```sh
luna-send -w 15000 -i -f luna://com.webos.appInstallService/dev/install \
  '{"id":"io.prysm.capture","ipkUrl":"/tmp/io.prysm.capture_0.1.11_all.ipk","subscribe":true}'
```

Run this installation command in an interactive SSH terminal (`ssh -t`). This
TV's `luna-send` can exit before delivering a response when stdin is closed.
The package setup helper keeps a pipe open and validates the activation reply,
so boot setup and Homebrew Channel execution do not require a terminal.

Stop any manually launched Prysm capturer or PicCap before the first run. Open
**Prysm Capture** on the TV:

1. If the service is unavailable, choose **Set up service**. To repair an existing
   setup, open **Service options → Set up or repair service**. This calls Homebrew
   Channel with a fixed command to run the Rust setup helper as root, register
   supervisor/capture permissions, and activate the native service.
2. Open **Prysm receiver** with OK, enter the IP and port, and choose **Use this
   address**. Open Capture or Compression to choose their values, then **Save changes**.
   The initial choices are both layers and LZ4. Hardware-specific CLI flags remain
   available in the standalone capturer; this setup app uses automatic backends.
3. Select **Enable sender** (or **Save and enable** if settings are unsaved).
   **Enable with TV** controls autostart, which defaults off. It arms the sender
   at future boots; keep it off until cold-boot and standby behavior is validated.
4. Close the TV app and enable **Screen sync** in Prysm's web panel. Prysm must
   have a webOS input listening on the configured IP/port.

Arrows move focus only. OK opens a setting; inside the dialog, arrows move between
choices and OK stages a choice. **Save changes** explicitly applies settings.
Back cancels an open editor, returns from service subviews, or asks before closing
the app. Text fields retain left/right cursor movement. Closing the app keeps the
sender's current state. **Enable sender / Disable sender** controls whether
the TV is available to Prysm. It does not indicate whether frames are being
captured: egui Stop or panel Off pauses capture while the sender stays enabled
for an automatic resume.

The UI follows LG's [simple-screen design principles](https://webostv.developer.lge.com/develop/guides/design-principles)
and [Magic Remote focus/activation model](https://webostv.developer.lge.com/develop/guides/magic-remote):
a status/action area and a settings list, large focus indicators, explicit activation,
and pointer support. There are no native dropdowns to accidentally edit while navigating.
It reuses the web panel's light background, lavender accents, triangular Prysm
wordmark, dark status card, ambient gradient, and five-color light spectrum.
Diagnostics and setup repair live under **Service options**; removing setup requires
confirmation and defaults to keeping the current setup.

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
release has a reproduced lost-wakeup hang, so process isolation remains in place. The standalone CLI also handles one connection, exiting if connection
fails or the receiver disconnects. The supervisor owns retries.

Version 0.1.4 was installed and checked on the target TV using an isolated Prysm
receiver and loopback-only output. Two idle → capture → idle cycles delivered
frames to the host and returned to fresh workers with no capture-device handles
open (the LG logging lock remained). Idle RSS returned to about 2 MiB each time;
active RSS was about 6.4 MiB at 320×180/15 FPS H.264. Closing Prysm cleared TV
connectivity. Saved settings/enabled state and the existing manual permission
mount were restored afterward. These are short lifecycle checks, not a long-run
memory or image-quality test.

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
While video is absent, the worker keeps sending UI frames on the same connection
without constructing a video client. With video-only source selection it waits
without sending UI. Video initialization resumes immediately when readiness returns.
Input context changes reset only the video client. Duplicate notifications do not
reset it; rapid loss/return transitions are retained until the worker consumes them,
including when readiness becomes unknown before delivery.

Native video open, preparation, and release have a five-second watchdog, including
release after acquisition errors. A hung reset, failed constructor,
or preparation that stays unready for ten seconds still triggers process recovery
with the existing bounded teardown and failure backoff. Ordinary video absence
reported by Luna does not start a restart cycle. Failed/malformed subscriptions
fall back to bounded capture retries and resubscribe after five seconds. Standalone
CLI capture retains its original retry behavior. Valid black frames never expire.

The service receives typed JSON events over a dedicated inherited pipe, separate
from vendor stdout and diagnostic stderr. `PRYSM_EVENTS_FD` identifies the private
descriptor; standalone CLI use needs no status channel. `PRYSM_CONTROL=1` enables
the supervisor-only stdin channel; commands never enter the receiver protocol. The supervisor clears connectivity on disconnect
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
only the Prysm service identity, with outbound access only to TV power. Cleanup
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
only the setup app's Luna caller identity. Receiver arguments are validated and
passed to `spawn` as an argument array without a shell.

Before uninstalling, open **Service options → Remove service setup… → Remove setup**. This
disables autostart, stops/joins the sender, removes Prysm's native role manifest,
and rescans Luna. The package manager removes the app/service files and their
generated service registration. Removing the IPK first can leave a harmless
guarded boot hook and stale native registration until cleanup/reboot; the IPK
does not have an automatic removal script. Saved receiver settings remain.

If VTCapture reports `LSRegisterPubPriv FAILED` or permission denied, validate
that Luna actually loaded the separate native manifest on this firmware. The package uses its own standalone registration; it does not apply a system
manifest overlay. Historical overlay procedures remain in the research archive.

## Verification

```sh
node --test packaging/webos/tests/*.test.js
python3 -m unittest discover -s packaging/webos/tests
cargo test --locked -p prysm-webos-service --all-features
cargo test --locked -p prysm-webos-capture -p prysm-webos-protocol -p prysm-capture
```

The current build passed native metadata/mode checks. Rust tests cover worker
ownership, TERM/KILL/reap, power gating, config persistence/rollback and mutation
identity. Browser tests cover the native subscription, reconnect, cancellation,
activation timeout and preservation of edits/focus. The isolated TV smoke test
also exercises Rust setup and activation without changing installed paths:

```sh
python3 packaging/webos/tests/native-smoke.py --host root@TV_IP --supervisor
# Optional bounded capture; all pixels are discarded on TV loopback.
python3 packaging/webos/tests/native-smoke.py --host root@TV_IP --capture
```

The following installed-package measurements predate the Rust service cutover:

Verified on LG 43QNED87A6D.BEUFLJP, webOS 25 / 10.3.2-34: IPK installation and
upgrade; noninteractive root service setup; `Active` power observation; five
complete 320×180 native NV12 frames using the packaged executable role; native
manifest cleanup without changing the manual capture overlay; video/UI mode
with LZ4 to Prysm's receiver; and panel Off → Screen sync with fresh frames.
The existing manual capture manifest mount was preserved throughout. Autostart
was kept off and no TV reboot or power transition was requested.

UI reconnect testing initially grew native device handles from 33 to 38 in one
process. With process isolation, the same two-connection test stayed at 24/24.
At the existing 640×360/30 fps setting, video/UI LZ4 sometimes hit the transport
write deadline; sustained throughput has not been tuned. Leave the sender stopped
until starting a viewing/test session; autostart remains off on the test TV.
The final resource-release check confirmed that Off replaces the connected
sender with a fresh waiting process without capture-driver libraries loaded.
Starts at 640×360/30 fps were intermittent. The TV sender and host pipeline were
left stopped; lower-rate/resolution testing is needed before unattended use.
With an active HDMI 1 source (3840×2160/50 Hz), the same configured capture
delivered 166 frames over 9.4 seconds, but three sender process IDs were observed
across the sampling interval. HDMI selection restored frame production; reconnects
still occurred at this capture setting. No claim of sustained 30 fps is made.

On the TV, verify setup with no capture running, Screen sync → frames, Off →
resource release, repeated start/stop, resolution changes, receiver restart,
standby/wake with Quick Start+ both enabled and disabled, cold boot/autostart,
and cleanup/uninstall. Also confirm that closing the TV app does not stop the
service and that a failed power subscription stays visible in status.

The native transport remains unauthenticated. Restrict the receiver port to
the TV on a trusted LAN, as for the standalone sender.

Version 0.1.8 corrects the supervisor role to allow outbound videooutput calls as
well as tvpower. A package regression check covers both destinations. The
installed worker now receives unavailable state on Home and defers video startup;
settings, enabled state, boot-hook state and capture-permission mounts were preserved.

The 0.1.8 Home → HDMI1 test kept the same worker and receiver connection. With
MAIN disconnected, about 72 seconds on Home produced 2,124 additional UI frames
without opening a video device or restarting. Video started in that worker on
returning to HDMI1; the user confirmed quick recovery. Other physical HDMI ports,
built-in apps and standby/wake remain unverified on hardware.

The reverse HDMI1 → Home test also passed: the active video device released,
the same worker remained alive, and 510 further UI frames arrived over 16 seconds
with the receiver connected and no error. Exact transition latency was not measured.

Version 0.1.9 corrects status-1 recovery: LG initialization/preprocessing failure
can finalize the native session and close its device. The worker marks that
session uninitialized and retries init → preprocess once per second on the same
driver, while UI frames continue. It retains the ten-second persistent-failure
limit, and native startup/reinitialization is covered by the five-second operation
watchdog. Process restart remains fallback for persistent failure or a native hang.
HDMI signal state was investigated but is not an additional production dependency.

The installed 0.1.9 wake test reproduced the initial preprocessing failure and
recovered by session reinitialization in 1.09 seconds, retaining the wake worker
and receiver connection. The user confirmed visible video within about two
seconds of the TV picture. Full OS cold boot and other physical HDMI ports remain
unverified; this test used suspend/resume with Freebox on HDMI1.

Version 0.1.10 uses absolute TV paths for setup tools: /usr/sbin/ls-control,
/usr/bin/timeout and /usr/bin/luna-send. The full reboot boot environment omitted
/usr/sbin from PATH, so setup previously failed before activating the native
service. The existing boot hook and saved autostart setting remain unchanged.

A final full OS reboot was verified with no Luna requests from the observer and
the TV web app kept closed. On 0.1.10, native setup succeeded and HDMI frames
were advancing by 66.54 seconds OS uptime. The user confirmed automatic capture.
The boot hook still follows earlier serial Homebrew hooks; service/worker startup
was about 65 seconds after OS boot. Startup ordering was left unchanged.

Version 0.1.11 corrects the verified default display capture path to limited-range
BT.709 throughout composition, encoding, and decoding. Wire protocol 3 rejects
older peers, so update the receiver and TV worker together. It was installed on
2026-11-01 with settings, autostart, boot hook and permission mount preserved;
640×360 H.264 frames were confirmed advancing in the updated receiver. See the
[colorimetry and deployment evidence](../../research/webos-capture/2026-11-01-colorimetry/README.md).
