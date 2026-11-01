# Event-driven video readiness investigation

Observed 2026-11-01 on LG webOS 25 / 10.3.2-34, with installed Prysm 0.1.6.
Read-only Luna subscriptions ran while the user manually switched HDMI1 → Home
→ HDMI1. Production capture and configuration were unchanged during this probe.
No pixels were collected. Selected status fields and relative timestamps are in
[luna-events.json](luna-events.json).

## Observed

`com.webos.service.videooutput/getStatus` with `{"subscribe":true}` accepted the
Prysm service identity and replied `subscribed:true`. The TV API metadata lists
this method in its public group. MAIN initially reported HDMI1, connected,
unmuted, 3840×2160 at 50 Hz.

On Home: MAIN first reported zero dimensions/frame rate, then disconnected.
The foreground notification identified `com.webos.app.home`.
On HDMI1: foreground changed at 32.79 seconds; MAIN connected with zero dimensions
at 32.80, then reported 3840×2160/50 Hz at 32.81. Duplicate status notifications
followed. Therefore `connected` alone is insufficient; changes must be deduplicated.

`com.webos.applicationManager/getForegroundAppInfo` also accepted subscriptions,
but app identity does not establish video availability (including playback inside
an already open app). `com.webos.service.extinputs.hdmi/getSignalState` rejected
a subscription without mandatory `pipelineId`. No HDMI control calls were made.

## Alternatives

1. **Smallest improvement:** subscribe in the existing Rust Luna supervisor.
   Run a UI-only worker while MAIN has no usable video; start the combined worker
   on a meaningful ready transition using the existing intentional-restart path,
   without the failure backoff. Keep the saved source setting unchanged. This
   avoids constructing the native video client on Home and repeated idle retries,
   but still briefly reconnects the receiver on transitions. Unknown or failed
   subscription state must retain bounded fallback behavior, rather than disable
   video permanently. Source=video must not silently become source=ui.
2. **Smoother, larger change:** pass readiness to the existing worker over a
   bounded control channel; defer native video construction while continuing UI
   capture and encoding on the same connection. The current pipe is events-only,
   so this needs a reverse channel and reconnect/configuration handling. Ordinary
   native lifecycle reuse could handle later source transitions, with an external
   watchdog for hangs. This cannot promise uninterrupted UI on a vendor hang.
3. **Native-only reinitialization:** existing lifecycle research demonstrated
   public stop/postprocess/finalize/init reuse and one successful retry after an
   induced missed wakeup. It also reproduced unbounded release and restart races;
   reuse alone cannot replace process recovery. See the previous lifecycle report.
4. **Separate video subprocess:** isolates video recovery from UI and transport,
   but adds frame IPC, buffering and synchronization. Defer unless uninterrupted
   UI during native failure becomes a requirement.
5. **Different native backend:** prior hardware investigation found DILE to be
   a stub on this firmware and GM absent. It is not a practical substitution here.

## Recommendation and remaining checks

Use videooutput status as an advisory readiness signal, with MAIN connected,
nonzero dimensions/frame rate and not muted as an initial candidate predicate.
Validate mute semantics before treating muted as unavailable: ordinary audio mute,
transitions, protected content and app-specific behavior must not suppress valid
capture. Treat this as source availability, not proof of capturable frames.

Implement alternative 1 first if a brief transition reconnect is acceptable;
choose alternative 2 if continuous UI capture is required. Retain the existing
native-error/first-frame watchdog as fallback, but do not run an unconditional
restart cycle while absence is confirmed. Restart only on relevant transitions
or actual capture failure, not every duplicate notification.

Only one Home/HDMI round trip was observed. Built-in apps, HDMI signal loss,
standby/wake, bus reconnect and notification-to-first-captured-frame latency remain
untested. The 20 ms observation concerns Luna events, not capture latency.
No implementation or additional deployment was made for this investigation.

Reference: [official OSE videooutput API](https://www.webosose.org/docs/reference/ls2-api/com-webos-service-videooutput/).
The OSE API is marked retired; the retail TV was tested directly rather than
assuming OSE support or schema compatibility.

## Implementation

Version 0.1.7 implements alternative 2: nonblocking supervisor-to-worker commands
on a private stdin pipe, delayed native video construction while MAIN is absent,
and video-client reset on source loss or context change. UI capture and encoding
stay on the same receiver connection. A five-second native-reset watchdog and
the existing fatal-error recovery retain process isolation. Duplicate events are
deduplicated and loss/return events are latched across supervisor ticks. No HDMI
port or app ID is hard-coded. Video mute alone does not mark a source unavailable.

The installed package was verified capturing HDMI with the receiver connected.
Regression tests include all HDMI ports, VDEC, rapid transitions, subscription
retry/cancellation, control-pipe delivery, and continued UI frames on one TCP
connection while initialization is gated. ARM Miri checks passed (23 tests, two
integration tests excluded by the existing Miri harness). Native hardware
Home/HDMI transition verification follows deployment.

### Deployment permission correction

During the 0.1.7 full-Home hardware test, foreground was Home and MAIN was
reported disconnected, but the worker kept its video device open. The installed
binaries and private stdin pipe matched the build. The supervisor role allowed
outbound calls only to tvpower; the earlier luna-send probe used a different
executable and did not validate this restriction. Version 0.1.8 adds videooutput
to the supervisor role and a package metadata regression test, observed failing
before the fix and passing afterward. Settings and existing capture mounts are
preserved during redeployment. This correction does not change the Rust binaries.

### Verified 0.1.8 Home → HDMI result

After the permission correction, the worker remained PID 13402 for the complete
Home-to-HDMI observation. On Home, MAIN was disconnected and no video capture
device was open for about 72 seconds; the receiver stayed connected and processed
2,124 additional UI frames. Returning to HDMI1 opened the video device and logged
video capture startup in the same worker, with advancing frames and no error.
The user confirmed that video appeared quickly. Summary:
[validation-0.1.8.json](validation-0.1.8.json).

This verifies deferred initial video construction and return to HDMI without a
worker restart or receiver reconnect. The subsequent reverse-transition check below verifies active-video teardown.
Physical ports 2–4, built-in apps and standby/wake remain unverified.
All HDMI port values and VDEC readiness are covered by parser regression tests.

### Verified 0.1.8 HDMI → Home result

The baseline worker was PID 13402 with HDMI active and its video device open.
After the user switched to full Home, the same worker had released the video
device. A further 16.13 seconds on Home advanced the receiver by 510 UI frames;
all samples stayed connected, capturing, and error-free. No worker restart or
receiver reconnect was required. This validates ordinary active-video teardown
as well as deferred startup. The exact transition latency was not measured.
Summary: [validation-0.1.8-reverse.json](validation-0.1.8-reverse.json).

### Warm transition timing and cold-boot distinction

A subsequent 0.1.8 warm round trip was traced with simultaneous foreground,
videooutput and supervisor-status subscriptions plus worker/device inspection.
HDMI foreground arrived at 31.384 s, usable MAIN geometry at 31.422 s, and the
supervisor published native video startup at 31.723 s. Native worker log times
put availability-to-startup at about 252 ms. PID 3162 remained unchanged and no
error or fallback occurred. The user reported video in under two seconds.
These are notification/native-start measurements, not exact first-visible-frame
latency. Trace: [warm-transition-timing.json](warm-transition-timing.json).

The user then clarified the slow case: cold boot → Home → HDMI, with black video
for ten seconds or more. This differs from the warm transition above. A boot-time
readiness timeout, early initialization while the subscription is unavailable,
or slower vendor startup remain hypotheses; no cold-boot cause is established.

### Reproduced suspend/resume startup delay

The user-described cold-boot sequence was observed as Suspend followed by Active;
the supervisor PID 6461 survived and no OS uptime reset was seen. After returning
to HDMI, usable MAIN geometry arrived about 3.25 seconds after the foreground
event. The new worker then initialized its video client, which remained in
preprocess status 1 for ten seconds. It exited, the supervisor waited five seconds,
and worker 7596 started video successfully. This reproduces the long delay on 0.1.8.
Trace: [suspend-resume-delay-trace.json](suspend-resume-delay-trace.json).

The Freebox may be waking through CEC after TV power-on. The read-only HDMI
`getSignalState` method accepts MAIN.context as pipelineId and currently reports
`videoSignalState: good`. This is separate from videooutput geometry. Its update
behavior and relationship to the failing initialization are being traced; no
production recovery changes have been made on the basis of this hypothesis.

### Session lifecycle correction (0.1.9)

The second wake trace showed HDMI signal state changing unknown → good alongside
MAIN geometry, while the worker still entered status-1 preparation and the
10-second/5-second process-recovery path. The user saw the Freebox boot logo on
the TV before Prysm displayed it. Trace:
[hdmi-signal-delay-trace.json](hdmi-signal-delay-trace.json).

Rechecking the existing lifecycle decompilation found that preprocessing failure
runs native cleanup/finalization. setFMT returns 1 for an invalid device FD or a
failed ioctl; repeatedly preprocessing a finalized session therefore cannot
recover simply because the external video signal becomes valid. Our Rust stage
remained Initialized, so the old retry loop omitted initialization.

Version 0.1.9 marks status-1 initialization/preprocessing results as an
uninitialized session and retries init → preprocess once per second using the
same driver object. Successful preprocessing clears the wait timer. The ten-second
persistent-failure deadline remains; startup/reinitialization uses the existing
five-second operation watchdog. Known video absence still gates initialization.
No new signal API permission or library dependency is added.

The fake vendor now models preprocessing closing the session. The regression
failed under the old retry logic, then passed after the stage correction. It also
covers initialization remaining unready once and succeeding on a subsequent
attempt without recreating the driver. Capture and service tests (47 and 36),
strict Clippy, ARM package build and package metadata checks passed.

### Verified 0.1.9 wake result

The same TV off/on → Home → HDMI1 sequence again produced preprocessing status 1.
This time the worker reinitialized its finalized session and started native video
1.091374 seconds later. The wake worker remained PID 19274 and stayed connected
through the recovery; no further worker restart, ten-second timeout or failure
backoff occurred. The user confirmed video within about two seconds after it
appeared on the TV. Settings and enabled/autostart state were preserved, and
receiver frames continued advancing (60 frames over a final two-second check).

[Trace](wake-reinitialization-0.1.9-trace.json),
[validation summary](validation-0.1.9-wake.json).
The temporary diagnostic watcher was stopped. A full OS cold boot, other physical
HDMI ports and long-run stability remain unverified; this was suspend/resume.

### Full OS reboot: setup PATH failure (0.1.10)

After a full reboot on 0.1.9, the TV showed no capture until the user opened the
Prysm TV app. Uptime was about 146 seconds when inspected. Saved autostart was
true and the recognized executable boot hook was present. Its setup log contained:

```
timeout: can't execute 'ls-control': No such file or directory
Error: Luna service rescan failed: exit status: 127
```

The installed tool is /usr/sbin/ls-control. Running the installed setup script with
PATH=/usr/bin:/bin reproduced the same error before any scan or service activation.
The currently running service/worker had started after the app was opened, which
explains why the UI launch recovered capture.

Version 0.1.10 uses absolute paths for ls-control, timeout and luna-send in the
Rust setup path. Normal service rescans share the absolute scanner path too.
The boot hook text is unchanged. The service tests (36 plus codegen), strict
Clippy, ARM build and package checks passed. Runtime capture logic is unchanged.

The deployed 0.1.10 setup passed the exact restricted-PATH runtime reproduction
([summary](validation-0.1.10-restricted-path.json)). On the next full reboot,
the boot setup log reported success and native HDMI capture started at about
65.6 seconds uptime; the user confirmed video appeared with the TV app closed.
However, an active diagnostic observer could have activated the service while
the temporary --setup process was present. That round is therefore not treated
as a clean end-to-end autostart verification. Those observers were stopped.

Homebrew runs init.d hooks serially, with 20-tvweb-services and 50-tvweb before
prysm-capture. The tvweb boot log showed roughly 17 seconds spent in that earlier
hook on this boot. No third-party hook or system partition was changed. A final
observer uses only process/filesystem reads and local receiver counters, sends
no Luna requests, and waits for an actual uptime reset plus successful setup,
HDMI device acquisition and advancing receiver frames.

### Clean full-OS boot verification (0.1.10)

The final observer sent no Luna calls: it read process/filesystem state and local
receiver counters only. An actual OS uptime reset was recorded. No service/worker
was present at uptime 47 seconds; by uptime 66.54 seconds, setup had returned
success, the native worker held /dev/video60, and receiver frames were advancing.
The setup status reported zero subscribers. The user kept the TV app closed and
confirmed capture started automatically, with a delay greater than 30 seconds
(the initial estimate of 90 seconds was approximate). This verifies autostart
without observer-triggered activation or opening the web app.

Subsequent process metadata placed service startup at about 64.4 seconds and
worker startup at about 64.6 seconds. Earlier serial Homebrew hooks run first;
50-tvweb logged 16 seconds from start to its final synchronous log message on this
boot. The remaining boot delay is predominantly before Prysm starts. No startup
ordering or third-party scripts were changed.

## Review fixes: ARM and native validation

The review fixes were cross-built with the relocated webOS SDK for
`armv7-unknown-linux-gnueabi`. The generated 0.1.10 IPK contains the exact worker
and supervisor binaries tested on the TV. Five focused ARM regression tests
passed there, covering reset delivery under unknown readiness, decoding, native
cleanup event ordering, preparation events, and supervisor recovery deadlines.

A temporary worker captured real LG video at 320×180/15 FPS on TV loopback.
Unknown readiness allowed capture; an explicit reset released and reopened video
in 0.60 seconds while retaining the worker and TCP connection. An isolated
supervisor then ran the worker with a temporary library wrapper that forwarded
real LG calls, injected an acquisition error after five successful acquisitions,
and blocked native release. Four frames reached the receiver before the fault.
The watchdog reported the timeout after 5.20 seconds and killed/reaped the worker
another 4.95 seconds later.

The first native attempt lacked the temporary Luna service descriptor; that test
setup was corrected before both native checks passed. Production capture was
restored after each attempt. Saved settings, the boot hook, and installed binary
hashes were preserved; temporary TV files and registration were removed. Host
frame processing advanced from 101075 to 101099 during the final check.

[Build hashes, ARM test results, native observations, and cleanup evidence](validation-review-fixes.json).

[Validation](validation-0.1.10-full-boot.json),
[passive trace](passive-full-boot-0.1.10-trace.json).
