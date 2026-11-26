# PicCap and hyperion-webOS: capture reliability evidence

Reviewed 25 November 2026. Companion to the [architecture report](REPORT.md).
This is a focused review of public issue discussions and a relevant merged
change, not an exhaustive tracker export or a reproduction on Prysm hardware.
No runtime changes or TV tests were performed. Shell access to GitHub failed;
the sources below were inspected through web retrieval and the browser.

**Main conclusion.** The issue history supports investing in resource arbitration,
explicit lifecycle ownership, frame provenance and qualified platform profiles.
It does not establish that replacing LG's wrapper will fix AirPlay or increase
capture throughput. A displayed failure category can obscure the actual failing
operation, and a workaround that helps one source can damage another.

**Evidence register.** “Observed” below means visible in the upstream record,
not independently reproduced here. Dates refer to the reports unless stated.

| Source and platform | Evidence and limits | Implication for Prysm |
| --- | --- | --- |
| [Hyperion #137](https://github.com/webosbrew/hyperion-webos/issues/137), C4, May 2025 | Logs show permission true, failed `VT_CAPTURE` allocation and init status 11, despite a permission-denied label. LG's capture service is also taking a surface-manager snapshot and releasing the resource. The author reports improvement after disabling AI picture features; the logs do not establish which feature caused the snapshot. | Distinguish policy refusal from allocation failure. Record resource events and competing capture activity. AI settings are test variables, not a proven universal cause. |
| [PicCap #93](https://github.com/TBSniller/piccap/issues/93), PS5 HDR, February 2025 | A live-source log exposes unrecognised `hdrALLM`; maintainers identify metadata handling, and the author confirms a supplied build fixes the problem. Earlier reinstall successes were retracted. Earlier no-data logs were taken with the diagnostic app foregrounded. | Preserve raw dynamic-range values and handle verified variants. Collect diagnostics without replacing the source being investigated. Do not infer the cause from an unrelated no-data event. |
| [Hyperion PR #105](https://github.com/webosbrew/hyperion-webos/pull/105), merged 11 March 2023 | The [diff](https://github.com/webosbrew/hyperion-webos/pull/105/files) adds backend initialization tracking, initialization on service start and cleanup on stop. It supersedes [PR #99](https://github.com/webosbrew/hyperion-webos/pull/99), whose discussion identified retained interfaces and missing cleanup around standby. A maintainer reports webOS 5 validation. | Strong evidence for lifecycle repair as a class of fix. Audit actual ownership and partial-init cleanup in Prysm; a merged historical change is not proof that all later restart reports share its cause. |
| [PicCap #59](https://github.com/TBSniller/piccap/issues/59), C8/webOS 4.4, October 2022 onward | The author reports first-start HDMI capture working but service restart/settings reload breaking it until reboot, while UI capture still works. Maintainer links the lifecycle work. Later comments include resolution-specific behaviour after an update. The thread does not demonstrate universal resolution. | Test first start and repeated reopen separately. Track UI and video health separately. A resolution workaround cannot establish whether the underlying failure is geometry, allocation or cleanup. |
| [PicCap #66](https://github.com/TBSniller/piccap/issues/66), 55SM8500, March 2023 onward | Conversely, video reportedly starts only after a settings change or service restart; UI capture works first. Later comments mix startup and autostart symptoms. No established root cause. | Similar visible symptoms can require opposite actions. Record service start, source readiness and permission arrival before choosing recovery. |
| [PicCap #22](https://github.com/TBSniller/piccap/issues/22), C1/webOS 6.2, January 2022 | Reporter says vtCapture works, but trying DILE leaves vtCapture unusable until reboot. Maintainer cautions that available libraries do not all work on every TV. This is an older single-platform report. | Backend initialization is potentially stateful. Avoid speculative fallback chains that initialize every discovered library. Require supported profiles and demonstrated cleanup. |
| [Hyperion #145](https://github.com/webosbrew/hyperion-webos/issues/145), B9/webOS 4.5, October 2026 | Reporter says DILE `CreateEx` fixes top-screen corruption but causes HDMI Dolby Vision black video with audio; the alternate no-freeze setting did not help. No maintainer-confirmed cause or fix in the reviewed thread. | Qualify quirks against source modes and transitions. “Fixes SDR corruption” is insufficient evidence for a global default. |
| [Hyperion #144](https://github.com/webosbrew/hyperion-webos/issues/144), 43LM6300/M3R, September 2026 | Reporter attributes DILE mapping failure to unaligned plane addresses and reports success with page-base alignment plus an offset. No reviewed merged fix or independent confirmation. | Validate mapping bounds, plane layout and address arithmetic. For V4L2, use driver-returned `QUERYBUF` mapping offsets; do not transplant a physical-address mapping workaround. |
| [Hyperion #59](https://github.com/webosbrew/hyperion-webos/issues/59), B6/Shield, 2021 | Reported cadence is about half the source rate, including 24 Hz content. Maintainer suspects synchronization waits; reporter says disabling the suggested wait was already the tested configuration. Root cause remains unresolved. | Measure each wait/acquire/copy/release stage and unique-frame cadence. This is not evidence that direct V4L2 automatically doubles fps. |
| [Hyperion #139](https://github.com/webosbrew/hyperion-webos/issues/139), 55UK6300/webOS 4.4.3 | About 8 fps reported across tested resolutions, without enough logs to identify the limit. | Treat low fps as a symptom. Distinguish acquisition cadence, duplicate frames, conversion and transport pressure before blaming resolution or CPU. |
| [Hyperion #81](https://github.com/webosbrew/hyperion-webos/issues/81), Quick Start resume | Report describes mosaic output after resume with Quick Start enabled and capture/vsync failures in logs. Suggestions in the thread do not establish a general fix in the reviewed evidence. | Resume needs fresh source/permission/resource validation. Test stale mappings and frame generations rather than assuming a surviving process has a valid capture session. |
| [Hyperion #68](https://github.com/webosbrew/hyperion-webos/issues/68), maintainer quirk tracker | Documents model-specific creation/crop quirks and power notifications whose processing field can change while the state remains Active, producing unwanted pauses. | Separate authoritative power state from transition hints. Make capture location, geometry and power semantics explicit profile properties. |

**What should change in the proposed design.**

Permission, resource ownership, source readiness and frame validity must remain
independent. In particular, allocation failure should not become permission
refusal merely because a wrapper printed that phrase. Preserve the raw result,
operation and correlated service facts. Do not label every status 11 as contention
either: the inspected binary establishes resource-acquisition failure, and the
upstream C4 log supplies one concrete concurrent-resource example.

Treat every successful start as a new capture generation. A source switch,
standby, revocation or completed teardown invalidates the old frame contract.
Partial initialization must retain enough ownership information to release what
was actually acquired. Cleanup failure remains observable; clearing a local
initialized flag cannot prove hardware or service resources were released.
Keep Prysm's established stop/close/idle/join order and worker isolation rather
than copying another project's thread order literally.

Backend choice should use a qualified tuple: SoC/board, firmware and binary
fingerprints, userspace ABI, backend, capture location and source mode. Library
presence and successful initialization are weak compatibility checks. Avoid
initializing a second backend after a policy refusal. Where fallback is valid,
prove that the previous owner has relinquished the resource first.

HDR handling needs two separate answers: what the TV says it is displaying, and
what colour representation the capture backend actually returns. Preserve unknown
metadata instead of silently treating it as SDR; do not automatically apply an
HDR transform to already tone-mapped capture pixels. The upstream parsing case
does not establish a current Prysm parser defect; that needs its own code audit.

Recovery should follow changes that can make capture possible: permission,
resource availability, source readiness or power state. Do not turn these reports
into a timer that continually reopens the same failing backend. If a platform has
no useful resource-availability event, document and measure a bounded fallback
policy before adding it.

**Acceptance scenarios derived from this review.** These are proposed experiment
criteria, not newly executed tests or a parallel task tracker.

| Scenario | Required observation |
| --- | --- |
| Cold boot versus repeated service stop/start and settings reload | Video recovers without reboot; old frames disappear; resources do not accumulate |
| Standby/resume with Quick Start on and off | Bounded shutdown; fresh session state; no mosaic or stale colour output |
| Permitted capture overlapping an LG screenshot or another capturer | Allocation/revocation is classified accurately; no service killing; recovery follows regained availability |
| SDR, HDR and Dolby Vision transitions on qualified sources | Picture remains intact; capture geometry and colour semantics remain correct |
| 24/30/60 Hz input with multiple capture rates | Unique-frame cadence and stage timings identify pacing limits and redundant waits |
| Video unavailable while UI remains capturable | Diagnostics say video unavailable; UI-only success does not masquerade as full capture |
| AirPlay desktop, personal photos and owned video | Background metadata correlates permission, mute, pipeline, allocation and frame outcomes without replacing the source |

No confirmed AirPlay fix was found in the reviewed threads. They improve the
diagnostic method and regression matrix; they do not show that AirPlay refusal
is necessarily content protection, nor that ordinary permission can be ignored.
The next useful AirPlay investigation remains the correlated observation defined
in the [binary investigation](README.md#concrete-next-experiments).
