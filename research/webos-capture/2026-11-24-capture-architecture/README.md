# webOS capture architectures and AirPlay

For the consolidated findings, backend design, performance assessment and
webOSbrew and OpenLGTV follow-ups, read the [25 November report](REPORT.md). The analysis below
retains the original binary-level evidence and experiment protocol.

Date: 2026-11-24. Status: static investigation, with earlier TV recordings
reviewed; **no new hardware experiment or deployment**. Implementation reviewed
at `8885aaf1`, in an isolated worktree. Later recovery changes in other worktrees
are outside this implementation review.

Platform of the supplied binaries: webOS TV 10.3.2 / Rockhopper, ARM32 userland
on an AArch64 5.4.268 kernel. Findings about their internals apply to those
binary hashes, not every LG TV. Maintained operational guidance:
[webOS](../../../docs/developers/webos.md).

## Recommendation

Keep the LG-backed adapter as the production baseline, but make permission,
resource ownership, frame availability and unsupported capabilities separate
outcomes. Investigate a **Prysm-owned V4L2 userspace backend**, behind an explicit
hardware profile, as the next architectural experiment. Do not start with a
kernel module or an automatic chain of alternative backends after a refusal.

There is a credible replacement opportunity: the current LG libraries use
standard V4L2 streaming operations plus LG-specific controls. Owning dequeue,
copy and requeue would address a concrete frame-ownership weakness, improve
diagnostics and potentially remove vendor thread-lifecycle failures. It does
not establish that AirPlay can be captured or that kernel calls become reliably
cancellable.

The next AirPlay experiment should distinguish an explicit pipeline policy from
a transient mute and from a stale client view. The existing evidence proves a
refusal, but does not identify which of those caused it. A permission-respecting
fix is plausible only if ordinary permission becomes available, or if a
different ordinary, permitted presentation path is involved. An explicit
refusal remains a refusal, including for personally recorded video.

## Evidence and corrections to the earlier findings

The owner's earlier `work/tv-libs/FINDINGS.md`, decompiled excerpts and
`airplay-recording` logs were inputs. They remain local and are not distributed
here. This review freshly decompiled selected functions in `libvt.so.1.0.0`,
`libvtcapture.so.1.0.0`, `libresource_mgr_client.so.1.0.0`,
`libhalgal.so.1.0.0` and `captureservice`, resolved ARM PC-relative string
literals, and inspected the Thumb jump table that the decompiler could not
recover. Binary hashes and function addresses are in [evidence.json](evidence.json).
No vendor binaries or decompiled vendor source are included in this report.

| Earlier statement | More precise conclusion | Evidence |
| --- | --- | --- |
| 17 means not ready | At init, 17 means the permission check refused capture. During acquisition it can originate from kernel `EACCES`. It is not an ordinary startup delay. | `initVtCapture`, `dequeueBuffer`, acquisition jump table |
| 11 means resource busy | At init, resource acquisition failed; contention is one possible cause. The wrapper also fails with missing client state or already-held resources. During acquisition, 11 represents dequeue `EAGAIN`. | `ResourceManagerClient::acquire`, `dequeueBuffer`, acquisition jump table |
| Permission is just a cached byte | The ordinary verdict is cached, but the service combines per-pipeline permission with video mute. The client also contains a vendor exception mechanism; Prysm should retain its own identity and use the ordinary verdict. | `PermissionData`, `publishPermission`, `Permission` |
| AirPlay mutes MAIN, therefore DRM | The recordings show AirPlay/VDEC and refusal. They do not contain the correlated permission/mute replies needed to prove the cause, and an app name does not establish DRM state. | Earlier recordings; service control flow |
| Every libvt/DILE path passes the same gate | Not established. This TV's `libvt` has a separate V4L2 implementation; sharing a device does not prove identical userspace checks. Kernel restrictions may still apply. No separate `libdile_vt` binary was supplied. | Fresh `libvt` decompilation |
| Query `getVideoMuteState` | The inspected service actually subscribes to `avmutemanager/listMuteStatus`, requesting `muteType: videoMute`. | `registerVideoMuteStateSubscribe`, resolved literals |
| Screenshot success proves video capture | The inspected display screenshot path can fall back to graphics-only capture and return success. A successful file is not proof that video was captured. | `DisplayCapture::displayCapture` |

### Permission is a useful diagnostic signal

`PermissionManager::publishPermission` emits both `status` and `coreStatus`.
In the inspected binary its ordinary effective verdict is:

`status = coreStatus && !videoMute`

`coreStatus` comes from the permission data associated with the current video
pipeline; authorised components can supply permission data. The mute observer
matches entries by the current pipeline's `contextId`. The MAIN selection comes
from the videooutput observer. This is more specific than treating any TV mute
as a capture refusal.

| Observed reply | Interpretation for diagnosis | Prysm behaviour |
| --- | --- | --- |
| `status=true`, `coreStatus=true` | Service currently permits ordinary capture; resource or device failures can still occur | One normal acquisition attempt when otherwise ready |
| `status=false`, `coreStatus=true` | Mute contributes the refusal in this service implementation | Wait for an effective permission change; record context and mute |
| `status=false`, `coreStatus=false` | Pipeline permission itself refuses capture; mute may also be active | Report capture unavailable; do not choose another backend to evade it |
| Missing, malformed, disconnected or stale reply | Permission unknown, not permission granted | Stop publishing affected video; show the observation failure |

These are **static field semantics**, not observed AirPlay replies. A service
subscription is also not an atomic lease: permission can change after its last
message and before a driver operation. Native refusal always wins.

The client constructor initially sets its cached verdict to true, then subscribes
asynchronously. Do not copy that default into a new backend. Wait for a valid
initial observation, and invalidate it on service loss or pipeline change.
The callback's parsing depends on the reply structure as well as the boolean;
a malformed response is not evidence that permission changed.

### Operation-specific native outcomes

The acquisition mapping was checked against actual instructions, not only the
decompiler's incomplete switch:

| Origin | Native condition | Visible result on this binary |
| --- | --- | --- |
| Init | Ordinary permission check returns false | 17 |
| Init | Resource-manager wrapper returns false | 11 |
| Init | Device availability check fails | 1, after internal cleanup |
| Buffer query | No current buffer, sentinel -1 | 2 |
| Dequeue/pump | `EAGAIN` (11), sentinel -5 | Buffer query returns 11 |
| Dequeue/pump | `ENODATA` (61), sentinel -4 | Buffer query returns 13 |
| Dequeue/pump | `EACCES` (13), sentinel -3 | Buffer query returns 17 |
| Buffer query | Sentinel -2, set by the no-callback branch of `onPolicyAction` | 12; a resource-policy interruption, not simply an empty queue |

Keep `(backend, operation, raw_code)` with every classified outcome. Do not apply
this table globally to other LG library families: the inspected `libvt` dequeue
wrapper itself uses different status numbers. Likewise, a resource acquisition
failure does not prove that a retry will eventually succeed.

## What the libraries actually own

### libvtcapture

The inspected path includes all of the following:

1. A Luna connection and permission subscription, resource-manager client and
   native pump thread are created with the driver.
2. Init checks permission, acquires a multimedia resource, takes its capture
   lock, opens `/dev/video60`, checks V4L2 capabilities and reads LG capabilities.
3. Preprocess sets format and capture-plane properties, reads negotiated
   properties, allocates/maps buffers, queues them and starts streaming.
4. The pump dequeues a frame, publishes its index and requeues it. The public
   current-buffer API returns pointers and sizes under an index mutex; the
   caller does not hold that mutex or a dequeue lease while copying pixels.
5. Stop/postprocess/finalize unwind capture state. Driver destruction signals
   and joins the pump **before** its later finalization and resource destruction.

Prysm should not simplify this to “open device, dequeue one buffer”. A direct
client needs negotiated format/planes, buffers, permission, resource arbitration,
revocation and ordered cleanup. A successful capability query proves none of
those later operations.

The destructor ordering and condition-variable worker explain why releasing a
partially initialized object deserves special attention. They identify plausible
hang sites, not the proven root cause of the recorded release hang. Existing
recordings establish that the failure happens; a stack trace with native symbols
or a controlled lifecycle experiment is still needed to identify the exact wait.

The lower-level `libresource_mgr_client` adds another blocking boundary:
`_acquire` sends a message and waits on a condition variable without an explicit
deadline at that wait site. Its destructor stops the connector and joins its
thread. A custom V4L2 client that retains this synchronous resource API therefore
does not automatically make open or close bounded. Preserve watchdog isolation
until the resource-client contract is also established; do not remove resource
arbitration to avoid the wait. `VtDriver::onPolicyAction` demonstrates that
ownership can be revoked after a successful acquire.

### Standard V4L2 versus LG-specific contracts

The following ioctl encodings were found in the ARM32 library. They are evidence
identifiers, **not portable constants to paste into a new backend**. Compile
against the target ABI's headers; pointer and time layouts can change encodings.

| Observed encoding | Standard operation | Observed use |
| --- | --- | --- |
| `0x80685600` | `VIDIOC_QUERYCAP` | Streaming + multi-plane capture capability check (`0x04001000`) |
| `0xc0cc5605` | `VIDIOC_S_FMT` | Capture dimensions; inspect returned format rather than assume it |
| `0xc0185647`, `0xc0185648` | `VIDIOC_G_EXT_CTRLS`, `VIDIOC_S_EXT_CTRLS` | LG capability, geometry and capture-plane payloads |
| `0xc008561b`, `0xc008561c` | `VIDIOC_G_CTRL`, `VIDIOC_S_CTRL` | LG frame-rate control |
| `0xc0145608` | `VIDIOC_REQBUFS` | Buffer allocation |
| `0xc0445609` | `VIDIOC_QUERYBUF` | Plane mapping information |
| `0xc044560f`, `0xc0445611` | `VIDIOC_QBUF`, `VIDIOC_DQBUF` | Buffer ownership exchange |
| `0x40045612`, `0x40045613` | `VIDIOC_STREAMON`, `VIDIOC_STREAMOFF` | Streaming lifecycle |

LG's capability payload is 40 bytes in the inspected `getDeviceCapabilityInfo`;
the public `vtCapture_capabilityInfo` also writes 40 bytes but **transforms the
internal fields**. Equal size does not make the two structures interchangeable.
The public header's dummy pointers are not decoded fields. Existing bead
`prysm-884` and its draft probe should incorporate this distinction.

The kernel extension describes scaling support, geometry, buffer count, planes
and pixel format. The library computes a frame-rate divisor from current source
rate and requested rate; a failed divisor-setting call is logged without always
preventing streaming. Requested fps is therefore not proof of delivered fps.

### libvt, DILE and graphics

Fresh `libvt` decompilation finds `VT_V4L2_*`, `VT_DILE_*`, texture/context
management and its own buffer lifecycle. On this firmware it opens the same
`/dev/video60`, uses the standard queue ioctls and LG controls. It is not simply
an alias for `vtCapture_*`. Replacing one vendor wrapper with another retains
vendor threads, ABI assumptions and shared driver risks.

Older `libdile_vt`/`libgm` backends remain possible compatibility work for older
TVs, based on upstream implementations, not proven targets in this review.
Select them for a qualified hardware family before capture starts, not because
the first backend refused a particular source. Upstream's approximate version
ranges are starting points, not a compatibility guarantee.

`libhalgal` is useful for the graphics plane. Its inspected entry points delegate
to `KADP_GAL_*`; surface creation copies 1056 bytes, supporting Prysm's ARM32
surface-size assertion. This does not establish the lower driver's completion
fence or coverage of decoded video. The local UI research already measures its
cost and cleanup behaviour. Replacing this small wrapper alone would not remove
the underlying graphics-driver risks.

## Challenge to Prysm's implementation

| Priority | Finding at `8885aaf1` | Consequence and proposed change |
| --- | --- | --- |
| High | `prepare()` labels 11 as Luna denial and 17 as not ready | Replace text-only interpretations with operation-specific typed outcomes; preserve raw status and cause |
| High | Most video failures reopen after a timer; missing library is not fatal in `is_fatal()` | Stop permanent failures. Wait on the relevant event for refusals/resources. Coordinate with the existing recovery redesign, not a second recovery machine |
| High | `VideoSignal` holds video availability and a context generation, but no capture permission | A recovery design waiting only for source changes can miss permission restoration within one AirPlay pipeline. Observe permission separately and wake on meaningful changes |
| High | `VideoFrame` borrowing suggests the driver cannot recycle its buffer before copy; vendor pump immediately requeues it | The borrow protects Prysm calls, not DMA/vendor writes. Correct the contract and validate coherent snapshots. Do not promote this pointer API to zero-copy downstream processing |
| High | The current-buffer API supplies no frame sequence/timestamp to Prysm | Repeated reads may count as new frames. Measure unique-frame cadence and age with a numbered test video; do not equate poll rate with fresh-frame rate |
| Medium | Mode limits advertise all even sizes to 1920×1080 and 1–60 fps | Separate configuration bounds from proven device support; query and validate negotiated output, backed by qualified profile data |
| Medium | NV12 limited BT.709 and post-tone-map semantics are assigned universally | Evidence is strong for the tested TV/path, not every SoC or dump point. Qualify format, range, matrix and HDR domain per backend/profile before converting to the fixed downstream contract |
| Medium | UI fallback and video share downstream frames | Preserve frame provenance and the reason video is unavailable. Menus lighting successfully must not be presented as AirPlay video working |
| Medium | Native C/Rust layout tests compare locally maintained declarations | Useful for consistency, but they do not establish vendor ABI compatibility. Add binary/profile evidence and hardware negotiation tests |

Relevant implementation:
[native wrapper](../../../crates/adapters/capture-webos-sys/src/vtcapture.rs),
[native loop](../../../crates/adapters/capture-webos/src/source/native.rs),
[video signal](../../../crates/adapters/capture-webos/src/video.rs),
[backend](../../../crates/adapters/capture-webos/src/backend.rs),
[settings](../../../crates/adapters/capture-webos/src/settings.rs).

Keep the current strengths: exception containment, one owning capture thread,
cross-process capture lock, retained library handles, checked plane geometry,
owned frame copies, newest-frame delivery, process watchdogs and ordered
stop/close/idle/join. A custom client does not make those protections obsolete.
The frame race demonstrates lack of a coherent-snapshot guarantee; it does not,
by itself, demonstrate an out-of-bounds read or prove visible tearing occurred.

## Alternative architectures

| Architecture | Main benefit | Main limitation | Recommendation |
| --- | --- | --- | --- |
| Hardened LG `vtCapture_*` adapter | Smallest production change; preserves LG arbitration and hardware integration | Vendor pump, cleanup and pointer-lifetime limits remain | Production baseline |
| Prysm V4L2 client with LG control/resource integration | Explicit buffer ownership; sequence/timestamp access; clearer errno and lifecycle | Private control ABI and arbitration still require qualification; no AirPlay permission guarantee | Best next experimental backend |
| `libvt` or legacy DILE backend | May cover another firmware/SoC family | Different native ABI, status codes and cleanup; same capture hardware on tested TV | Explicit compatibility profiles only |
| LG screenshot service | Useful diagnostic reference; service owns native implementation | Service/file overhead; image encoding depends on format (historical raw outputs exist); unknown sustained rate, possible graphics-only success | One-shot comparison, not default 30 fps transport; see [follow-up](REPORT.md) |
| LSM screenshot / graphics capture | Menus and graphics that the compositor actually owns | Separate video planes may be absent; OSE API is not a retail-TV contract | UI fallback or diagnostics |
| GStreamer/appsink in an owned playback pipeline | Copy or sample permitted frames if the decoder exposes them | Owning playback does not ensure decoded-frame access; LG direct-output paths may expose none; pre-display colour/timing differs | Conditional integration, not an established TV capture path; see the [follow-up](REPORT.md) |
| Source-side Mac capture while mirroring | Avoids LG capture ABI for a user-authorised source | AirPlay/TV latency must be measured; DRM exclusions remain; not all AirPlay modes mirror local pixels | Practical experiment for unprotected Mac mirroring |
| Separate AirPlay receiver, e.g. UxPlay on a companion device | Owned decode/render pipeline and accessible unprotected frames | Changes the receiver experience; not equivalent to LG AirPlay or DRM streaming | Optional product direction, not an LG capture fix |
| HDMI capture | Existing external-source architecture | Cannot see internal LG apps; only ordinarily capturable HDMI; delay/format limits | External sources only |
| Camera looking at the panel | Sees actual displayed light independent of TV capture ABI | Placement, room reflections, exposure, geometry and latency calibration | Broadest physical compatibility, separate experience |
| DRM/KMS writeback | Standard Linux abstraction where actually exposed | No evidence this TV exposes a usable connector; compositor ownership and fences matter | Inventory first; no modesetting experiment yet |
| New kernel module | Could expose a new hardware path if genuinely absent | Per-SoC driver knowledge, kernel/config ABI, crash risk; cannot grant permission | Not justified by present evidence |
| Vendor-provided regional colour statistics | Could avoid full-frame transport | No authorised, suitable spatial-statistics interface established | Ask LG if pursuing an integration |

A generic framebuffer, Wayland screenshot or GPU readback is not necessarily the
final panel image: hardware video planes may never enter that buffer. Likewise,
reading colours rather than storing a video does not change the permission of
the underlying capture operation. Histogram symbols exist, but a global
histogram lacks the spatial information needed for edge lighting; regional
coverage and permitted access would need separate evidence.

For full-package ambient lighting, keep computation close to the frame and use
small qualified resolutions. The existing resolution research is a better guide
than maximising capture resolution. For a future companion design, sending
timestamped regional colours could reduce traffic further, but it changes the
processing/preview contract and is not a prerequisite for fixing capture.

## Proposed backend contract and promotion criteria

Keep the blocking capture object behind the existing adapter seam. Subscription
delivery, cancellation and frame pacing belong in the existing runtime/host
layers; pure pixel processing remains in `prysm-core`. Do not scatter retry
policy across the backend, worker and supervisor.

Represent the following facts separately: permission (`Unknown`, `Allowed`,
`Refused`), current source generation, negotiated frame description, native
resource state, and last new-frame time. Preserve typed failures for unsupported
ABI/mode, resource acquisition failure, permission refusal, transient empty queue
and unconfirmed cleanup. Feed these into the already planned recovery model.

For a direct backend, the intended ordinary lifecycle is permission observation
and normal resource acquisition, capability/format negotiation, mapping/queuing,
streaming, dequeue, **copy while the buffer is owned**, then requeue. Publish only
the newest owned frame. Process stale dequeued frames without building a queue
downstream. Do not assume a supported zero-copy or DMA-BUF export path.

Use nonblocking dequeue and a stop wakeup only if the driver actually supports
them. Retain process isolation: a timeout around a blocking ioctl does not cancel
the operation, and a process in uninterruptible kernel sleep may not be reaped
promptly even after SIGKILL. No replacement capturer may race a still-owning
process. Never use thread cancellation inside LG's native calls.

Promote a backend only after it demonstrates correct permission revocation,
normal multimedia resource arbitration, stable frame ownership, format/colour
agreement, bounded ordinary stop and repeated reopen without TV side effects.
Qualify at least a second distinct SoC/firmware family before describing it as
portable. A single successful frame is a feasibility result, not a reliability
result.

## Concrete next experiments

These are planned, not run. Hardware testing requires a free TV and the usual
isolated probe identity, shared capture lock, bounded duration and cleanup.
Do not change source, power, manifests or playback automatically during a
read-only observation session. Keep only metadata by default.

### A. Identify the AirPlay refusal

Observe the three feeds concurrently with host monotonic receive timestamps,
their initial replies, service-loss events and raw error payloads:

```text
luna://com.webos.service.capturepermission/checkPermission
  {"subscribe":true}
luna://com.webos.service.avmutemanager/listMuteStatus
  {"subscribe":true,"muteType":"videoMute"}
luna://com.webos.service.videooutput/getStatus
  {"subscribe":true}
```

The first two are grounded in this binary's subscription code; live schema and
role access are still unverified. A denied subscription is a diagnostic result,
not a reason to modify LG policy. Record the full mute array and correlate by
pipeline `contextId`, not arrival order alone. Also record power, Prysm's actual
operation/status, backend and worker PID. Ignore stale callbacks from a previous
subscription generation.

| Controlled case | What it distinguishes |
| --- | --- |
| Home / no video | UI-only baseline and permission while idle |
| Known working Chromecast or HDMI with an owned test clip | Working video path baseline |
| AirPlay Mac desktop mirroring with animated numbered colour patches | Mirroring without a protected media application |
| AirPlay personal photo, then personal video | Still-image versus video/pipeline differences |
| AirPlay app-directed playback of the same owned clip | Mirroring versus remote playback differences |
| Pause, resume, end playback; return to the working source | Permission restoration without source change; clean recovery |
| An ordinarily played protected sample, if desired | Refusal/blanking only; no capture alternatives after a denial |

Interpret outcomes before attempting a new backend:

- `coreStatus=false` throughout: the effective policy is the blocker; report it.
- `coreStatus=true`, mute-driven refusal that later clears: test permission-event
  resume in the same pipeline without periodic reopen.
- Service says allowed but init says refused: investigate subscription freshness,
  identity, callback parsing and event ordering. Do not force the cached value.
- Init succeeds but no frame/driver denial follows: investigate the ordinary
  device outcome and source route; an init success is not a video guarantee.
- Permitted screenshots show only menus: verify video-plane provenance before
  declaring the screenshot path a solution.

### B. Validate the frame-ownership concern

Use an owned test clip with a visible frame number and abrupt alternating edge
colours. Compare copied-frame coherence, duplicate rate and LED timing at
320×180 and 640×360. A duplicated static scene is not a stall: use the encoded
frame number for this experiment. Capture age, p50/p95 timing, copy cost, CPU,
unique-frame cadence and stop duration matter more than polling fps.

Do not “fix” this by taking an undocumented mutex or adding volatile reads.
Neither establishes DMA ownership. A V4L2 prototype should hold a dequeued buffer
until its owned copy is complete, then requeue it, and demonstrate that the
driver honours that ownership.

### C. Qualify a replacement without changing policy

First establish the target headers, negotiated controls, ordinary permission
observer and normal resource client. Inventory read-only capabilities before
attempting allocation. The existing generic Linux capture adapter is a source
of buffer-validation patterns, not a drop-in TV backend: it lacks LG's control
and resource contracts.

Then, on a permitted test source with the installed capturer gracefully stopped,
run one isolated bounded stream and clean teardown. Follow with repeated
open/close, source changes, permission loss, resource competition and standby
only in an explicitly arranged hardware session. Stop after an unconfirmed
release; do not launch repeated probes against a possibly held device.

Device identity should include board/SoC, OS build, kernel, userspace ABI,
library hashes, V4L2 driver/card/device caps, mode/format and observed colour
domain. Profiles select verified implementations; firmware updates invalidate
unproven assumptions. Keep a successful profile's evidence instead of building
a growing list of unexplained quirks.

## Sources and validation limits

- [Linux QUERYCAP and capability flags](https://www.kernel.org/doc/html/latest/userspace-api/media/v4l/vidioc-querycap.html)
  identify the standard availability check. A capability bit is not permission.
- [Linux buffer ownership and layout](https://www.kernel.org/doc/html/latest/userspace-api/media/v4l/buffer.html)
  provide the intended queue contract; LG driver conformance still needs testing.
- [Linux V4L2 UAPI header](https://github.com/torvalds/linux/blob/master/include/uapi/linux/videodev2.h)
  supplies ioctl names. Target headers, not current upstream numeric encodings,
  must be used in a probe.
- [Hyperion webOS](https://github.com/webosbrew/hyperion-webos) documents separate
  video/graphics and legacy backends. Its compatibility ranges are not our tests.
- [LG OSE videooutput](https://www.webosose.org/docs/reference/ls2-api/com-webos-service-videooutput/)
  controls routing/scaling; it is not itself a frame-export contract.
- [LG OSE compositor capture](https://www.webosose.org/docs/reference/ls2-api/com-webos-surfacemanager/)
  documents video-plane limitations and secured-content refusal. OSE is not
  evidence that the same API is supported on every retail TV.
- [Dev Manager screenshot issue 419](https://github.com/webosbrew/dev-manager-desktop/issues/419)
  reports a legacy service-name and explicit-size compatibility difference;
  useful test input, not a measured result here.
- [Apple ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit)
  is the source-side capture API candidate.
- [UxPlay](https://github.com/FDH2/UxPlay) is a separate receiver candidate and
  explicitly does not support Apple video DRM.
- [Linux KMS writeback](https://docs.kernel.org/gpu/drm-kms.html#writeback-connectors)
  requires exposed driver support and proper synchronization; availability on
  this TV has not been established.
- Local prior research: [lifecycle](../2026-11-01-lifecycle/README.md),
  [UI capture](../2026-11-12-ui-capture/README.md),
  [colour interpretation](../2026-11-01-colorimetry/README.md),
  [resolution tradeoffs](../../processing/2026-11-12-resolution-detail/README.md).

This review changes no runtime code and claims no AirPlay fix. It narrows the
unknowns enough to test the right cause and gives a concrete reason to explore
owning the userspace queue lifecycle, without pretending that lower-level access
is permission or that one TV establishes compatibility across LG's range.
