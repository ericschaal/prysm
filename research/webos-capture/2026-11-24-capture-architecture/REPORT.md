# Prysm webOS capture: findings and recommendations

25 November 2026 · Research synthesis and proposed design · No new TV tests

**Recommendation.** Harden the existing LG-backed adapter and evaluate a
Prysm-owned, permission-aware V4L2 userspace backend on explicitly qualified TVs.
The strongest justification is coherent frame ownership, clearer failures and
better lifecycle control. Higher performance and AirPlay compatibility remain
unproven. A new kernel module is not justified by the evidence available.

This report combines the [binary investigation](README.md), its
[evidence manifest](evidence.json), earlier capture recordings and webOSbrew
and OpenLGTV research, plus a [PicCap/Hyperion issue review](UPSTREAM-ISSUES.md).
The implementation review covers commit `8885aaf1`; it is not an audit
of subsequent recovery changes. The supplied binaries represent webOS 10.3.2 /
Rockhopper, ARM32 userspace on an AArch64 5.4.268 kernel. Their internal behaviour
must not be generalised to all LG TVs. Current operational guidance remains in
the [webOS developer guide](../../../docs/developers/webos.md).

**What is established, and what remains uncertain.**

| Finding | Evidence and confidence | Consequence |
| --- | --- | --- |
| LG capture uses standard V4L2 streaming plus private controls | Static inspection of the supplied `libvtcapture` and `libvt` binaries | A userspace replacement is technically plausible, but a generic Linux webcam backend is insufficient |
| Ordinary capture permission combines pipeline policy and video mute | Inspected service computes `status = coreStatus && !videoMute` | Observe effective permission separately from source availability |
| AirPlay capture was refused | Earlier recordings; cause not correlated with permission and mute replies | Neither DRM nor a fixable mute transition has been established as the cause |
| Vendor buffer pointers lack a demonstrated snapshot guarantee | Pump publishes an index and immediately requeues; callers receive pointers without a dequeue lease | Rust borrowing alone cannot prevent DMA/vendor reuse during copying |
| Native release has plausible blocking sites | Pump joins and resource-client waits identified statically | Preserve process isolation; exact recorded hang cause is still unproven |
| Screenshot success can mean graphics-only success | Inspected display-capture fallback | Preserve video/UI provenance; menus lighting correctly do not prove video capture |
| Compatibility cannot be inferred from a library name | webOSbrew documents private ABI changes with unchanged names/versions | Use qualified profiles, runtime checks and behavioural tests |

The frame-ownership finding is a missing guarantee, not proof that visible tearing
or an out-of-bounds read occurred. Likewise, identifying a possible wait site does
not establish that it caused an observed hang.

**How the LG capture stack works.**

The inspected library owns much more than pixel access: Luna permission
observation, multimedia resource acquisition and revocation, the device,
negotiated format and private controls, mapped buffers, a pump thread and cleanup.
Both inspected video libraries reach `/dev/video60` on this TV, but they have
different wrappers and status conventions. The device path is an observation,
not a cross-model contract.

The standard queue operations are `REQBUFS`, `QUERYBUF`, `QBUF`, `DQBUF` and
`STREAMON/OFF`. Private LG controls cover capture location, capabilities, geometry
and rate. The public and internal capability payloads are both 40 bytes in the
analysed binary, but the wrapper transforms their fields. Matching sizes do not
establish interchangeable layouts. Compile standard ioctls from target headers;
do not copy ARM32 numeric encodings into an architecture-independent backend.

Native errors also depend on the operation. Init status 17 indicates ordinary
permission refusal, whereas acquisition status 17 can originate from `EACCES`.
Init status 11 means resource acquisition failed; acquisition status 11 maps to
`EAGAIN`. Status 12 can represent a resource-policy interruption. Preserve
backend, operation, raw code and cause alongside a typed classification.

**AirPlay: diagnose before changing the capture path.**

The next useful experiment is a time-correlated recording of ordinary
`capturepermission/checkPermission`, `avmutemanager/listMuteStatus` with
`muteType: videoMute`, and `videooutput/getStatus`. Exact requests are in the
[investigation protocol](README.md#concrete-next-experiments). Live schema and
access under Prysm's identity still need validation. Match mute records to the
selected pipeline's context rather than treating any mute as a refusal.

| Observation | Interpretation | Recommended response |
| --- | --- | --- |
| `coreStatus=false` | Pipeline policy refuses capture | Report unavailable; do not switch backends to evade the refusal |
| `coreStatus=true`, effective status false | Mute contributes to refusal in this implementation | Wait for a meaningful permission event; resume once allowed |
| Permission becomes allowed without a source change | Source-only recovery can miss a valid transition | Wake recovery on permission changes too |
| Service allows capture, native init refuses | Possible stale state, identity difference or event-ordering issue | Correlate replies and native operations; do not overwrite the verdict |
| Init succeeds but frames fail | Permission alone did not establish a working stream | Classify device, resource and frame outcomes separately |

Compare a known working source against Mac desktop mirroring, personal photos,
personal video and app-directed playback of the same owned clip. Include pause,
resume and exit. These distinguish presentation paths without assuming the
content's ownership determines the TV's permission policy.

A fix may be possible if permission legitimately returns and Prysm misses it, or
if a different supported presentation path is ordinarily capturable. Existing
evidence does not establish either outcome. No patch to permission checks,
identity impersonation or protection-disabling module is part of this proposal.

**Design of a permission-aware V4L2 backend.**

Keep the adapter a plain blocking object. Runtime/host code owns subscriptions,
scheduling and recovery policy; pure colour processing stays in `prysm-core`.
Use the existing recovery design rather than adding another retry mechanism.

Represent permission (`Unknown`, `Allowed`, `Refused`), source generation,
resource ownership and negotiated format as separate facts. Start with unknown
permission. The inspected vendor client initially caches true before its
subscription completes; that is not a safe default to reproduce.

The proposed lifecycle is:

1. Select a qualified platform profile and obtain a valid permission observation
   for the current source generation.
2. Acquire the normal LG multimedia resource and shared capture lock. Resource
   refusal is distinct from permission refusal.
3. Open the qualified device and negotiate capabilities, mode, planes, stride,
   buffer sizes and colour interpretation. Validate returned values.
4. Allocate, map and queue buffers, then start streaming.
5. Dequeue, validate metadata and copy into Prysm-owned storage **before
   requeueing**. Publish only the newest eligible frame. Do not retain driver
   pointers for downstream processing.
6. On observed refusal, service loss, source invalidation or resource revocation,
   stop publishing affected frames, discard pending stale frames and unwind the
   native session. Recovery requires a relevant event or explicit user action.

The intended ownership exchange follows the
[V4L2 queue contract](https://www.kernel.org/doc/html/latest/userspace-api/media/v4l/vidioc-qbuf.html).
Nonblocking dequeue reports an empty queue as `EAGAIN`; that is not a reason to
reopen the device. Driver conformance and readiness behaviour still require TV
tests. Bound any queue-draining work so continuous production cannot starve stop
or permission handling.

A separate permission subscription is **not an atomic capture lease**. Checking
again before publication reduces stale output but cannot guarantee instantaneous
revocation; native service/driver enforcement remains authoritative. If the
supported enforcement contract cannot be established, do not promote the direct
backend for that profile.

Retain watchdog isolation and the existing stop/close/idle/join ordering. Reusing
LG's synchronous resource client retains its blocking boundaries. A userspace
timeout does not cancel an in-flight kernel call, and killing a worker does not
prove resources were released. An unconfirmed release must prevent a competing
replacement capturer from starting.

**What webOSbrew changes in the recommendation.**

The [Wayland guide](https://www.webosbrew.org/develop/guides/wayland/) describes
private request opcodes changing without an interface-version increment.
SDL-webOS resolves device-provided interface descriptors and request names at
runtime. This is a useful compatibility pattern where introspection exists; it
does not establish reflection support for LG private V4L2 structures. Direct
retrieval of the linked implementation failed during this review, so this claim
is documentation evidence, not an independent source audit.

The [native media guide](https://www.webosbrew.org/develop/guides/native/media/)
documents a DirectMedia ABI change across webOS 4/5 despite an unchanged SONAME,
and separate SS4S implementations. It also describes exported Wayland windows as
video placement/binding mechanisms. “Exported” does not establish readable pixels.

[Media Basics](https://www.webosbrew.org/develop/guides/media-basics/) explains
separate hardware video and UI layers and SS4S's direct-output backends, which do
not return decoded frames. This corrects the earlier GStreamer/appsink proposal:
owning the playback pipeline is insufficient unless the selected decoder exposes
usable frames. Software decoding is not an assumed economical substitute.

The [library inventory](https://www.webosbrew.org/develop/guides/library-version/),
[machine-readable data](https://www.webosbrew.org/develop/caniuse/data/all.json)
and [ELF/IPK compatibility tools](https://github.com/webosbrew/dev-toolbox-cli)
can support dependency checks. Symbol availability must remain separate from
structure-layout, permission and frame-behaviour qualification. Several guides
are explicitly incomplete; treat them as leads backed by target evidence.

**OpenLGTV follow-up: tools, historical context and contention.**

The [OpenLGTV landing page](https://openlgtv.github.io/) points modern webOS work
to webOSbrew. Much of its older material concerns Saturn and early Broadcom TVs.
Its historical [VPU page](https://github.com/openlgtv/wiki/wiki/VPU) describes
distinct video/graphics processing and a mixer controlled through HAL APIs.
That supports investigating plane provenance, but its fixed resolution examples
and hardware descriptions are not modern cross-TV guarantees.

[epk2extract](https://github.com/openlgtv/epk2extract) is useful for offline
firmware analysis. It supports several firmware/filesystem formats and LG `.sym`
debug symbols, including function names and addresses. Extraction support depends
on the input format and, for encrypted EPKs, the required material being available.
It does not establish that current firmware contains useful symbols. No firmware
or symbols were extracted in this follow-up.

The practical recommendation is to assemble a small, provenance-tracked corpus
from lawfully available firmware or owner-authorised library copies. Compare
capture libraries, permission services, resource clients and associated metadata
across distinct SoCs and firmware revisions. Record hashes, exported symbols,
structure evidence, error mappings and lifecycle differences. Match any debug
symbols to the exact binary before using their addresses. This can narrow ABI
profiles offline, while hardware tests still establish operational behaviour.
Retain vendor material locally; publish only the derived compatibility evidence.

[buildroot-nc4 release history](https://github.com/openlgtv/buildroot-nc4/releases)
shows webOS userland, Wayland-extension and glibc compatibility work, plus emulator
targets. Use it to pin reproducible probe dependencies, not as proof that the
SDK matches a TV's private capture ABI. An emulator can exercise application
logic; physical capture timing and resource behaviour still require a TV.

The related community project
[webos-kernel-drivers](https://github.com/throwaway96/webos-kernel-drivers)
organises modules by kernel architecture, version and SoC. It is primarily a
serial-driver collection, not a generic capture implementation. It points to
[LG's source portal](https://opensource.lge.com/product) for kernel sources and
toolchains. Use matching source packages, where available, to investigate UAPI
and compatibility handling before more decompilation. Availability of the private
capture implementation has not been established. This strengthens the case for
separate kernel/userspace ABI identification; it does not establish a universal
capture module or replace userspace permission/resource integration.

[Informatic's development notes](https://gist.github.com/Informatic/1983f2e501444cf1cbd182e50820d6c1)
document screenshot modes and raw RGB/RGBA/YUV422 output alongside image formats,
with different service names on different TVs. The notes identify webOS 3.8 as
their original target. This qualifies the earlier overhead argument: compressed
image encoding may be avoidable if raw output works on the target. Service/file
overhead, layout, sustained cadence and actual video coverage remain unmeasured.
Use one-shot captures as controlled references, not a presumed streaming backend.

[webos-vncserver](https://github.com/Informatic/webos-vncserver) explicitly limits
capture to UI layers and reports conflicts with Hyperion/PicCap and LG's capture
service. This is additional evidence against treating GPU readback as full-screen
video capture. Its service-stopping workaround is not a Prysm recovery strategy:
Prysm should respect ownership and diagnose contention, not kill LG services.

[PicCap's known issues](https://github.com/TBSniller/piccap#known-issues) report
capture dropouts associated with AI Picture Pro, AI Brightness and related
features. This is a maintainer report, not a verified mechanism on the inspected
TV. Add the active picture features and concurrent capture applications to test
records. During a separately arranged A/B test, correlate dropouts with resource
revocation, permission and frame events; restore any changed settings afterwards.
Do not automatically disable picture features or label every dropout a policy
refusal.

The added investigation priorities are therefore offline ABI comparison and
controlled resource-contention tests. No reviewed OpenLGTV source establishes
an AirPlay fix, a permission-preserving universal capture API or a measured speed
advantage. This was a documentation/repository review, not a new driver audit.

**PicCap and Hyperion: implications from upstream failures.**

The [issue evidence register](UPSTREAM-ISSUES.md) distinguishes logged failures,
maintainer findings, merged changes and unresolved user reports. Three findings
particularly strengthen the architecture recommendation:

- [Hyperion #137](https://github.com/webosbrew/hyperion-webos/issues/137) shows
  permission granted but capture-resource allocation failing alongside an LG
  snapshot operation. Its permission-denied label is misleading. Permission and
  allocation must remain separate outcomes.
- [PR #105](https://github.com/webosbrew/hyperion-webos/pull/105) is a verified
  merged lifecycle change that initializes backends on start and cleans them up
  on stop. Restart and resume failures deserve ownership analysis before retries.
- [PicCap #93](https://github.com/TBSniller/piccap/issues/93) links a reported HDR
  problem to an unrecognised metadata value and confirms a supplied fix. Capture
  failure, colour interpretation and diagnostic-source changes can be confused.

Other reports describe backend switching damaging a previously working path,
creation quirks helping SDR while breaking Dolby Vision, half-rate capture and
resume corruption. They justify a transition-based acceptance matrix, not blanket
workarounds. A qualified V4L2 backend must still handle all these categories.
No reviewed issue establishes an AirPlay fix or a performance advantage for it.

**Alternatives and their place in Prysm.**

| Architecture | Assessment |
| --- | --- |
| Harden existing LG adapter | Best immediate baseline; correct classifications, permission events, provenance and frame-contract claims |
| Own V4L2 userspace backend | Best focused replacement experiment; still needs LG controls, arbitration and enforcement |
| `libvt` / legacy DILE | Compatibility options for qualified families, not automatic alternatives after denial |
| Wayland/UI/screenshot capture | Diagnostic or graphics fallback; full video coverage and sustained performance unproven |
| Owned TV player / GStreamer | Viable only with an actual frame-output contract or permitted display capture |
| Sender-side colour extraction | Promising optional Mac-mirroring architecture; transport regional colours with timestamps and measure display delay |
| Alternative native AirPlay receiver | Concrete research path, but receiving/playing video does not establish capture access |
| External HDMI capture | Applicable to ordinarily capturable external inputs, not internal TV apps |
| Camera observing the panel | Independent of private TV APIs; requires exposure, geometry, ambient-light and timing calibration |
| KMS writeback / new kernel module | No demonstrated need or supported path on the inspected TV; do not prioritise |

[UxAir](https://github.com/depius/uxair-webos) provides an existing UxPlay-based
receiver with Starfish and NDL variants. Its
[port notes](https://github.com/depius/uxair-webos/blob/main/docs/PORT-WEBOS10.md)
report earlier C4/webOS 10.3.1 testing, while distinguishing that from validation
of the current NDL build. This is playback evidence, not proof of Prysm capture.
An optional comparison can use the same unprotected clip and ordinary permission
observation. The project does not establish support for protected playback.

**Performance expectations.**

A direct backend still uses the TV's capture engine. Removing a wrapper does not
raise hardware rate or resolution limits. Its plausible gains are fewer redundant
polls/copies, clearer frame freshness and less scheduling variation; none has
been measured here. Correct ownership may require retaining a copy rather than
pursuing zero-copy prematurely.

For ambilight, optimise freshness and stable panel-to-LED timing before peak fps.
Start comparisons at 320×180 and 640×360, using the same source, colour pipeline
and output settings. Requested fps is not delivered unique-frame cadence: the
inspected library derives a divisor and can continue after a rate-setting failure.

[V4L2 timestamps](https://www.kernel.org/doc/html/latest/userspace-api/media/v4l/buffer.html)
have driver-defined clock and sampling semantics. They must be checked before
being used as capture age, and are not panel-presentation timestamps by default.
Measure actual panel-to-LED delay separately, for example with a high-frame-rate
camera observing both during numbered colour transitions.

| Measure | Decision it supports |
| --- | --- |
| Numbered-frame coherence and duplicate/drop rate | Whether ownership and freshness improve |
| Unique-frame cadence; p50/p95 frame age and panel-to-LED delay | Whether synchronization improves |
| CPU time, copy/conversion cost, memory and resource growth | Whether overhead falls without instability |
| Colour agreement across SDR/HDR and capture locations | Whether a faster path still produces correct light |
| Stop duration, repeated reopen and revocation behaviour | Whether the replacement is operationally reliable |

**Recommended order and acceptance gates.**

| Priority | Work | Evidence required to proceed |
| --- | --- | --- |
| 1 | Correct native outcomes and permission-driven recovery | Clear typed causes; no repeated reopen on unchanged refusal; same-pipeline permission restoration handled |
| 2 | Resolve the AirPlay diagnosis | Correlated permission, mute, pipeline and native-operation record for controlled unprotected cases |
| 3 | Validate current frame ownership and freshness | Numbered-frame coherence and cadence measurements, with provenance preserved |
| 4 | Build one qualified V4L2 prototype | Normal arbitration/enforcement, validated layouts, coherent copy-before-requeue, clean stop and resource release |
| 5 | Compare and extend | Reproducible timing/CPU/colour results, then validation on a second distinct SoC/firmware family before claiming portability |

These are research and promotion gates, not permission to defer soundness work
past 1.0. The project has designated this capture work as release-gating.
Production selection should change only after the evidence supports it; a
prototype's first successful frame is insufficient.

Relevant existing work is tracked in `prysm-uu8` (permission/AirPlay), `prysm-2zv`
(frame ownership/freshness), `prysm-d8j` (V4L2 prototype), `prysm-884`
(capabilities) and the existing recovery stream. This report does not assert
their current implementation status. Website research is recorded in
`prysm-gyq`; synthesis in `prysm-0fr`; the OpenLGTV and upstream-issue follow-up
in `prysm-b6z`.

No runtime code was changed, no backend was implemented or benchmarked, and no
AirPlay fix is claimed. The recommendation is to own the parts of userspace
capture that improve correctness while retaining LG's permission and resource
contracts, then let controlled measurements decide whether replacement pays off.
