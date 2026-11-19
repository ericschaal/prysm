# TV deployment history

**Status:** historical deployment notes, 2026-11-01–04. Claims and settings below
refer to the version described; they do not describe the current release as a whole.
**Platform:** project LG webOS 25 TV. Short checks do not qualify sustained performance
or other models/firmware. Some original local evidence was not committed.
**Current instructions:** [TV setup](../../docs/users/webos.md) and
[developer lifecycle](../../docs/developers/webos.md).

Version 0.1.31 and the matching ARM64 receiver were installed on the project LG
and Pi on 2026-11-04. All 41 installed TV app and service files matched the
IPK. The test exercised four-way remote navigation and Back, paired the deployed
phone UI using the TV QR, and confirmed that profile selection left the decoder
idle and the lights off. During playback, changing red, green, blue and the
Coast split pattern produced the expected receiver output on both edges.
Across 77 sampled frames, none were black and the TV reported no stall, seek
or decoder reset. The phone rejoined after reload and Stop released the decoder.
The sender reconnected, WLED was off, and the Pi settings file containing all
three profiles matched its pretest checksum. Local evidence is in
`work/tv-deploy-031/` and the Pi rollback is in `/var/backups/prysm/tv-deploy-031`.

Version 0.1.30 keeps one H.264 stream open across calibration patterns. On the
project LG, the receiver sampled 61 preview frames across solid and split-color
changes with zero black samples. The TV reported no stall, seek, or player reset
during those transitions. The pending-color indicator appeared during switches
and cleared during playback. The receiver's saved settings matched the pretest
snapshot. The 0.1.29 profile-selection screen also stayed quiet until Response.

Version 0.1.28 redesigns the receiver-address and QR screens. Both layouts passed
local checks at 720p and 1080p for focus, target size, overscan margins and
space for the system keyboard. On the LG, the receiver address filled in from
the sender, Back returned to address entry, and a freshly rendered QR decoded
from the TV screenshot and paired with the receiver. Exiting restored focus
to Match your lights. Receiver settings and profiles matched the pretest
snapshot; the lights stayed off.

Version 0.1.27 adds color intensity, six selective hue controls, and a mixed-color
check. The local workspace and browser checks passed before the receiver, web UI,
and TV package were updated on 2026-11-04. The LG played the three muted patches
and all three split-field clips at 1280×720 capture. On clay, reducing intensity
moved mean top-edge output from RGB (186, 90, 67) to (123, 115, 103). On red,
shifting Red hue toward Yellow moved it from (214, 1, 0) to (144, 90, 0) while
the decoded TV patch held red. All three split fields produced distinct left
and right LED output. These are receiver samples, not optical measurements.

The phone rejoined after reload and cancelled the draft. Both saved profiles,
the active selection and all other receiver settings matched the pretest file
byte-for-byte. The TV sender remained connected and WLED was off. The receiver
rollback is in `/var/backups/prysm/color-steps-20261104` and local evidence is
in `work/color-steps/`.

Version 0.1.26 adds TV/phone calibration pairing, looping static-color patterns,
and the footer spacing fix. Deployed on 2026-11-03 together with the ARM64 receiver
and embedded web UI. The receiver's existing TOML settings were migrated to JSON,
preserving 1280×720 at 30 fps, processing values and LED layout; the user's saved
`Calibration 1` profile was retained and activated unchanged. TV native executable
hashes matched the build, and sender settings, enabled state and boot hook were
preserved. No TV system partition was written.

Local checks, browser tests and Docker smoke tests passed before deployment.
The deployed stack passed phone pairing, saved-profile selection, reload/rejoin,
static cyan playback/capture, cancellation and Stop. Five decoded preview frames
confirmed 1280×720 capture; corrected mean LED RGB was 0, 198, 181. After cancelling
the draft, settings matched the deployment input exactly, WLED was off, the
receiver was idle, and the TV sender remained connected without an error.
Receiver rollback files are in `/var/backups/prysm/calibration-20261103`; local
logs, the prior TV package and app backup are in `work/full-stack-deploy/`.
Temporary uploads and the debugger tunnel were removed. This was a bounded
functional test; standby, reboot, sustained performance and optical accuracy
were not requalified. Color intensity, selective hue correction and mixed-color
checks remain future work in `TODO.md`.

Version 0.1.25 refactors the TV React app into components and hooks, simplifies
its TV layout, and removes the redundant disable confirmation. The service now
publishes video readiness so the UI stays **Waiting for video** when capture is
requested without an active source, including across receiver reconnects. The
sender remains enabled. Deployed on 2026-11-02 with all 21 installed payload files
verified and saved settings preserved. On-device UI checks covered navigation,
dialogs, disable/enable, and 24 stable waiting-state samples over 12 seconds.
Restoring HDMI 1 resumed hardware H.264 capture: the receiver processed 450 frames
in 15.01 seconds, with no pipeline error. This is a short recovery check;
standby, reboot and sustained performance were not retested. Local evidence is
in `work/webos-video-wait-deployment/` (not distributed).

Version 0.1.23 retains only Combined and Video only. DILE/GM/HAL, UI-only
capture and CPU blending are removed. Old UI-only configuration is rejected;
delete the saved config before reinstalling and configure the receiver again.
Deployed with the old config deleted and fresh settings saved; all 21 installed
files verified. Capture waits on Home without active video. See the
[cleanup audit](2026-11-02-encoder-latency/single-tap-cleanup.md).

Version 0.1.22 uses the native blended NV12 tap for **Both** capture, avoiding
separate UI capture and CPU composition. Video-only and UI-only retain their
existing paths. UI fallback remains available while video is absent or pending;
opening an unsupported blended tap falls back to separate layers. The package
also includes native UI-call/copy timings and a `VtcaptureBlended` backend label.
Deployed on the project TV with all 21 payload files verified and existing
settings preserved. A 30-second live sample averaged 12.9 ms per iteration at
30 fps, with 61% average budget headroom and no skipped deadlines.
[Qualification and deployment](2026-11-02-encoder-latency/combined-deployment.md).

Version 0.1.21 connects hardware `rvenc` directly to `appsink` and pairs each
picture with its current input metadata. This removes the parser's one-frame
wait while keeping DMA ownership, H.264 validation and software fallback.
Deployed on the project TV after staged qualification; all 21 installed files
match the IPK. The user requested starting the sender to the current computer;
the receiver address was updated and the sender is enabled, connected and
capturing with hardware H.264. Other settings were preserved. See the
hardware details (`apps/prysm-webos-capture/HARDWARE-H264.md` in the tag
`pre-1.0-consolidation`).
Deployment evidence (local evidence at `work/webos-encoder-latency/deployment/evidence/deployment.json`; not distributed).

Version 0.1.20 keeps hardware DMA buffers mapped and uploads native NV12 directly,
without an intermediate padded image. RGB/NV16 conversion remains in cached RAM.
Eight encoder phase timings and a Grafana hardware phase panel expose the costs.
The hardware-enabled package is installed on the project TV; all 21 files match
the IPK and hardware H.264, receiver, metrics, autostart and enabled state were
preserved. The receiver was idle during deployment.

A controlled TV baseline → updated → baseline benchmark at 640×360/30 FPS found
native NV12 encode wall time improved from 13.52–13.75 ms to 12.49 ms, with CPU
time during encode calls down from 3.25–3.29 ms to 2.01 ms. This is a short
synthetic benchmark, not total worker CPU or live end-to-end latency. Output wait
remained dominant at 12.13 ms; that version retained one-frame lookahead. Full hardware
qualification passed. See the
hardware details (`apps/prysm-webos-capture/HARDWARE-H264.md` in the tag
`pre-1.0-consolidation`)
and deployment evidence (local evidence at `work/webos-hardware-dma/deployment/evidence/deployment.json`; not distributed).

Version 0.1.19 passes native video directly to the encoder whenever the UI is
fully transparent. Visible UI still uses RGB composition, with capture checked
every frame. Deployed with hardware support included; the active settings remain
software H.264, both layers, metrics and autostart enabled. All 21 installed files
match the IPK and the receiver reconnected normally.

On the project TV at 640×360/30 FPS, a 20-second live sample with fully transparent
UI averaged 30.1 ms per iteration (90% of the 33.3 ms budget), compared with the
39.1 ms pre-deployment sample (117%). Capture achieved 29.6 FPS; output was 28 FPS
because the unchanged 2 Mbit/s encoder rate control skipped some frames. This is
a short measurement of current content, not a sustained benchmark. Seven pixel
and H.264 transition/color tests also passed directly on the TV.
Deployment evidence (local evidence at `work/webos-transparent-ui/deployment/evidence/deployment.json`; not distributed)
and timing sample (local evidence at `work/webos-transparent-ui/after-measurement.json`; not distributed).

Version 0.1.18 deploys the Rust GStreamer bindings with `--hardware-h264` on the
project TV. All 21 installed package files match the IPK, and the restarted
service reports `capabilities.hardwareH264: true`. Saved settings were preserved;
capture remains disabled with LZ4 selected and no receiver configured.
Deployment evidence (local evidence at `work/webos-gstreamer-bindings/deployment/evidence/deployment.json`; not distributed).

Builds use software H.264 by default. Pass `--hardware-h264` to include the
experimental hardware backend and enable its service configuration choice.

The deployed version 0.1.17 included both H.264 backends. Its TV app
selects `h264-software` or `h264-hardware`; the former `h264` value was removed.
For a clean installation, remove service setup, uninstall the app, and delete
`/var/lib/prysm-capture/config.json` before installing this package. Deployed
on 2026-11-01 after uninstalling 0.1.16 and deleting its saved configuration.
Installed app/service files match the IPK; the app opens with factory defaults,
capture disabled and no saved config file.
Deployment evidence (local evidence at `work/webos-hardware-integration/deployment-0.1.17.json`; not distributed).

Version 0.1.16 exports numeric requested FPS for the Grafana capture-budget gauge.
Deployed on 2026-11-01 with both ARM binary hashes verified, settings and metrics
opt-in preserved, and frames progressing. The host now exposes requested FPS and
last decoded-frame time; its initial idle state and resumed capture were verified.
[Deployment evidence](2026-11-01-readiness/validation-0.1.16-health-overview.json).

Version 0.1.15 adds a saved **Prometheus metrics** checkbox, defaulting off for
new and older configurations. Deployed on 2026-11-01; toggling on/off/on changed
the actual TV endpoint, restarted the worker, persisted the setting, and resumed
capture each time. Metrics were left enabled for Grafana; other settings were preserved.
[Deployment evidence](2026-11-01-readiness/validation-0.1.15-metrics-toggle.json).

Version 0.1.14 restores the supervised worker’s Prometheus exporter on port 9101.
It was deployed on 2026-11-01 with both binary hashes verified, settings preserved,
and capture progressing. Both monitoring targets were UP; the reloaded Grafana
dashboard matched the repository and both encoder panels returned live TV data.
[Deployment evidence](2026-11-01-readiness/validation-0.1.14-metrics.json).

Version 0.1.13 makes the capturer worker-only. It was deployed on 2026-11-01;
both installed binaries matched the ARM build. The worker ran without arguments
using the supervisor's typed startup environment, and receiver frames advanced
with no service error. Saved configuration, enabled state, boot hook, and
permission mounts were preserved. The receiver wire protocol remains unchanged.
[Deployment evidence](2026-11-01-readiness/validation-0.1.13-worker-only.json).

Version 0.1.12 shares typed configuration and worker IPC between the native
processes and centralizes the wire version. It was deployed on 2026-11-01 with
saved settings, enabled state, boot hook, and permission mounts preserved.
Installed binary hashes matched the ARM build; capture resumed without an error
and host frame processing advanced. Wire version remains 3.
[Deployment evidence](2026-11-01-readiness/validation-0.1.12-maintainability.json).

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

## Earlier validation and transitions

 Rust tests cover worker
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
the TV on a trusted LAN.

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
[colorimetry and deployment evidence](2026-11-01-colorimetry/README.md).

## Earlier bounded lifecycle check

Version 0.1.4 was installed and checked on the target TV using an isolated Prysm
receiver and loopback-only output. Two idle → capture → idle cycles delivered
frames to the host and returned to fresh workers with no capture-device handles
open (the LG logging lock remained). Idle RSS returned to about 2 MiB each time;
active RSS was about 6.4 MiB at 320×180/15 FPS H.264. Closing Prysm cleared TV
connectivity. Saved settings/enabled state and the existing manual permission
mount were restored afterward. These are short lifecycle checks, not a long-run
memory or image-quality test.
