# Supervisor validation history

**Status:** historical staged and installed checks on 2026-11-01.
**Platform:** project webOS 25 (10.3.2-34) TV, ARM soft-float SDK.
The sections below refer to different versions; superseded UI fallback and
older Node cutover notes do not describe current production behavior.
**Current contract:** `apps/prysm-webos-service/README.md` (in the tag `pre-1.0-consolidation`) and
[lifecycle/permissions](../../docs/developers/webos.md).

## Initial verification

On 2026-11-01 the smoke test passed on 192.168.18.10, webOS 25
10.3.2-34, using the existing ARM soft-float SDK. It passed every check listed
above, including the 30-second automatic shutdown. Idle probe RSS was 1856 KiB;
this is not a measurement of the future complete supervisor. The production
JavaScript service remained running and its configuration was unchanged.

Host unit tests, strict Clippy (including codegen), formatting and the ARM build
passed. Regenerating the bindings reproduced the checked-in files byte-for-byte.
Actual web-app bridge calls and changing power notifications remain unverified;
the smoke test observes power without turning the TV off or on.

## Supervisor verification (2026-11-01)

The extended supervisor passed 30 host tests plus the codegen test, 31 Linux tests
(including actual parent-death SIGKILL), strict Clippy for all targets/features,
ARM soft-float builds and byte-for-byte codegen checks. Pinned Miri passed 20 tests
on both ARMv7 and x86-64; filesystem and subprocess tests use their native harness.

The isolated TV test now has two supervisor modes:

```sh
python3 packaging/webos/tests/native-smoke.py --host root@192.168.18.10 --supervisor
python3 packaging/webos/tests/native-smoke.py --host root@192.168.18.10 --capture
```

Both passed on the same webOS 25 TV. The fake worker ignores TERM, exercising the
five-second KILL/reap deadline. The real worker test registers its own temporary
capture role and validates at least three raw 320x180 frames at a requested 15 Hz
through a TV-local loopback receiver. Pixels are discarded on the TV; only the
frame count and status leave it. This is a bounded capture/ownership check, not a
sustained-rate, video-freshness or image-quality result. Probe RSS after reactivation
was 2004 KiB in the final setup/capture test. Both tests also checked Rust setup/activation, mutation identity, saved sandbox
settings, cleanup, subscriptions, shutdown/reactivation and expiry. The final
capture run also checked the initial `subscribed: true` acknowledgement; browser
tests exercise reconnect and cancellation while preserving edits and focus. All temporary artifacts
were removed; installed Prysm, settings, boot hooks, power and input were preserved.

Process tests cover the inherited event descriptor, fragmented/invalid/oversized events, EOF,
stderr pressure, spawn failure, scanner failure/timeout, hook rollback, mutation
serialization, config restart and no replacement before reap. Routine polling is
bounded; unexpected owner destruction kills and delegates reaping to a thread,
never blocking the GLib thread. SIGKILL cannot force immediate reaping of a task
in uninterruptible kernel sleep. The measured native hangs were reapable.

The worker prechecks both allowed Luna registration prefixes before video
construction. This prevents the measured registration-denial leak, without
promising outbound permission or subsequent native success. A constructor error
is fatal to the worker, bypasses automatic backend fallback and session retries,
and emits the existing typed failure event.

The supervisor also subscribes to videooutput MAIN-sink status, without filtering
by app or HDMI port. It sends readiness and context resets through the worker's
private nonblocking stdin pipe. The worker defers video initialization while the
source is absent and continues UI capture on the existing receiver connection.
Source=video waits without UI frames. Duplicates are ignored and rapid source
transitions are latched until delivered. Subscription failures retain the bounded
capture fallback and retry the subscription after five seconds.

Native open, preparation, and release operations emit start/completion events,
including release after an acquisition error. A five-second watchdog detects a
hung operation and recycles the worker. Successfully
constructed clients that remain unready for ten seconds also fail before teardown;
known source absence no longer causes periodic restarts. Process retry/backoff and
Stop cancellation remain under the supervisor's control.

## Current cutover and installation validation

There is one native package and one shared build/linker configuration. The old
Node service, Node setup/activation helpers, JS rollback build, crate-specific
build scripts and system-manifest overlay templates have been removed. Saved
configuration and the guarded boot-hook text/path remain compatible with the
previous installation. Cleanup keeps saved settings and supervisor activation
while removing capture permissions.

Version 0.1.5 was installed on 2026-11-01 through the TV app installer. The
native executable was confirmed running as root, observing Active power, and
loading the existing saved configuration unchanged. The actual open TV web app
confirmed its PalmServiceBridge status subscription to the native service.
Sender and autostart remained disabled, the existing boot-hook state and manual
capture-permission mount were preserved, and temporary installer files were removed.

Version 0.1.6 subsequently upgraded 0.1.5 in place, preserving saved settings,
enabled sender state, boot-hook state and the manual capture-permission mount.
On Home, the ten-second readiness timeout caused worker exit and restart.
Returning to HDMI1 recovered Freebox video automatically without a manual
restart; the user confirmed visible video and receiver frames kept advancing
with the same worker. Standby/wake and cold boot remain unverified.

Version 0.1.7 was deployed with the same saved settings, enabled sender state,
boot-hook state and capture-permission mount. Installed HDMI capture and receiver
connectivity were confirmed; transition validation is recorded separately.

Native service registration does not guarantee immediate restart after a crash.
Recovery remains subsequent Luna activation or the opt-in boot hook until an
external TV supervisor is verified. Building this package does not deploy it.

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
