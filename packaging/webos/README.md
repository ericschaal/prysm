# Prysm Capture for rooted LG TVs

The IPK installs a TV setup app (`io.prysm.capture`) and a JavaScript supervisor
(`io.prysm.capture.service`) alongside the Rust ARMv7 capturer. It requires a
rooted TV with Homebrew Channel's root execution and boot hooks enabled. The
initial hardware target is the existing LG webOS 25 capture setup. Installation,
the standalone Luna role, root service, active-power observation, cleanup, and
panel stop/start have been verified there. Cold boot and standby/wake remain
unverified; autostart is disabled on the test TV.
There are no npm dependencies in the TV app or service. `webos-service` and Node
are provided by webOS. webOS 25 provides Node 16.20.2; see LG's
[JS service documentation](https://webostv.developer.lge.com/develop/guides/js-service-basics).

## Build and install

Install the [webOS CLI](https://github.com/webos-tools/cli), which supplies
`ares-package`, and the relocated TV SDK described in the
[capturer README](../../crates/capture/webos-capture/README.md). From the repo root:

```sh
npm install --global @webos-tools/cli@3.2.6
WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
  ./packaging/webos/build-ipk.sh
```

The script requires Python 3.9+ and `ar`, rebuilds the native binary, includes its license notices, and creates
`target/webos-ipk/io.prysm.capture_0.1.4_all.ipk`. The version comes from
`app/appinfo.json`. The package suffix follows the
webOS app packager convention; the included executable is ARMv7 soft-float.
The final archive has root-owned files/directories with no group/other write
permissions. The packaging check rejects install scripts, links, special files,
and payload paths outside the Prysm app/service/package directories. This fixes
the CLI's writable generated-JavaScript/service-directory defaults before elevation.

With a configured ares TV device, install using `ares-install --device TV
target/webos-ipk/io.prysm.capture_0.1.4_all.ipk`. Alternatively copy the IPK to
`/tmp` on the TV and install from a root shell:

```sh
luna-send -w 15000 -i -f luna://com.webos.appInstallService/dev/install \
  '{"id":"io.prysm.capture","ipkUrl":"/tmp/io.prysm.capture_0.1.4_all.ipk","subscribe":true}'
```

Run this installation command in an interactive SSH terminal (`ssh -t`). This
TV's `luna-send` can exit before delivering a response when stdin is closed.
The package setup helper keeps a pipe open and validates the activation reply,
so boot setup and Homebrew Channel execution do not require a terminal.

Stop any manually launched Prysm capturer or PicCap before the first run. Open
**Prysm Capture** on the TV:

1. If the service is unavailable, choose **Set up service**. To repair an existing
   setup, open **Service options → Set up or repair service**. This calls Homebrew
   Channel with a fixed command to elevate the supervisor, register native capture
   permissions, and activate it.
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
state. The standalone CLI also handles one connection, exiting if connection
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
available in diagnostics. The app polls status every three seconds.

When vtcapture reports that video is not ready during preprocessing, the sender
keeps that client and retries preparation once per second. TV UI frames continue
while video is unavailable; video capture starts when the source becomes ready.
The pending-video path was checked on the target TV with a minute of UI-only
frames and a separate sustained HDMI/Home/HDMI run. These checks do not establish
long-run stability across every input and standby mode.

The service receives typed JSON events over a dedicated inherited pipe, separate
from vendor stdout and diagnostic stderr. `PRYSM_EVENTS_FD` identifies the private
descriptor; standalone CLI use needs no status channel. There are no lifecycle
flags or transport callbacks. The supervisor clears connectivity on disconnect
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

Setup uses Homebrew Channel's existing `elevate-service` mechanism for the JS
supervisor, including its power-state subscription. This mechanism adjusts the
supervisor's generated Luna service permissions; running as root alone is
insufficient. See the [upstream implementation](https://github.com/webosbrew/webos-homebrew-channel/blob/main/services/elevate-service.ts).

Native capture has a separate, narrower executable role:

- `/media/developer/apps/usr/palm/services/io.prysm.capture.service/capture.role.json`
  names the packaged `prysm-webos-capture` executable and allows capture-client
  and media-resource-manager identities, with outbound access to capture,
  capturepermission, and media services.
- `/var/luna-service2/manifests.d/io.prysm.capture.native.json` registers that role.
  Setup calls `ls-control scan-services`. LG's capture manifest is not changed.
- `/var/lib/webosbrew/init.d/prysm-capture` restores elevation and native
  registration before activating the supervisor at boot. The hook backgrounds
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
that Luna actually loaded the separate native manifest on this firmware. The
older [temporary overlay procedure](../../crates/capture/webos-capture/README.md#temporary-luna-capture-permissions)
is a diagnostic reference, not something this installer applies automatically.
Do not leave that older temporary overlay active during package validation.

## Verification

```sh
node --test packaging/webos/tests/*.test.js
python3 -m unittest discover -s packaging/webos/tests
cargo test --locked -p prysm-webos-capture -p prysm-webos-protocol -p prysm-capture
```

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
