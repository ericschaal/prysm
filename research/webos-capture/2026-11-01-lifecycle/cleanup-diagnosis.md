# Capture cleanup diagnosis — 2026-11-01

Tested on the existing webOS 25 / 10.3.2-34 TV, ARM32 applications on its
AArch64 kernel. Findings below are specific to these library builds, not all LG TVs.

## Findings and changes

### HAL library lifetime caused the repeated UI handle growth

Eight open/capture/drop cycles reproduced **five additional device descriptors per
cycle**: `/dev/rtd-logger`, `/dev/gdma_hal_gal0`, `/dev/auth`, `/dev/console`, and
`/dev/se0`. The wrapper's `/dev/gfx` mapping disappeared on drop; four mappings
owned by the native stack remained per cycle (three `/dev/se0`, one `/dev/rtkmem`).
Three cycles without acquiring a frame reproduced the same growth.

Keeping `libhalgal` loaded with a process-local `LD_PRELOAD` control stopped the
growth across 20 cycles, without changing initialization, capture, or destruction.
The production fix therefore retains the successfully loaded HAL API with a
process-lifetime `OnceLock<Arc<HalApi>>`. Every surface is still destroyed and
unmapped; initialization still runs on the capture thread. Failed library loads
remain retryable. Native surface pointers continue to make captures non-Send.

Read-only disassembly explains why adding a finalize call would not fix this TV:

- `libhalgal`: `HAL_GAL_Init` at `0x18d8` calls `KADP_GAL_Init`;
  `HAL_GAL_Finalize` at `0x1960` calls `KADP_GAL_Finalize`.
- `libkadaptor`: `KADP_GAL_Init` at `0x65140` caches an opened device descriptor
  in library global storage, opening it when the stored value is `-1`.
- `KADP_GAL_Finalize` at `0x65224` is just `movs r0, #0; bx lr`.

Unloading loses this bookkeeping without closing the descriptors. Retaining the
library bounds the retained native state; it does **not** release all five handles
when capture stops. Process exit remains the known way to reclaim them. This is
not evidence of a video kernel-driver leak. No undocumented finalizer was called.

### Asynchronous shutdown could crash video initialization

The original signal-hook flag handler plus SIGTERM during repeated video startup
reproducibly caused SIGSEGV. UI-only SIGTERM completed cleanly. Preloading the video
library did not prevent the crash. Explicit runner logging confirmed SIGTERM was
sent before the fault.

A temporary SIGSEGV diagnostic recorded a write to `0xf9` in
`VtCapture::prepare`, just after `prysm_vt_preprocess` returned: ARM register `r5`,
holding the wrapper's `self`, was zero at `strb r0, [r5, #249]`. The exact mechanism
that corrupts this preserved state is unresolved; do not describe it as a proven
kernel bug or a proven vendor-only bug. The diagnostic recorded only fault/register
metadata, not captured pixels or a memory dump.

`shutdown::listen` now blocks SIGINT/SIGTERM before any native or metrics threads
start. Those threads inherit the blocked mask. A dedicated `sigwait` thread sets
the existing atomic cancellation flag. Native calls finish before ordinary
cleanup runs. The waiter exits after the first shutdown signal; process exit
reclaims it if startup fails or the receiver disconnects first. The supervisor's
SIGKILL escalation remains available if a native call never returns.

The regression test checks inherited signal masks and delivery of both SIGINT
and SIGTERM to the waiter. Hardware cancellation passed after this change,
including combined capture without preload instrumentation.

### Video constructor failures still retain resources

Without a role for the temporary executable, `vtCapture_create` throws
`LSRegisterPubPriv FAILED`. Eight attempts retained **eight eventfds**, one per
failure. Holding the video library loaded did not prevent this. No driver handle
is returned to Rust, so our normal release sequence cannot reach that partial
construction.

Follow-up allocation tracing **corrects the earlier attribution to
`LS2Interface`**. The failure occurs earlier, in the resource-manager dependency:

1. `VtDriver::VtDriver` constructs its resource-manager client before its own
   `LS2Interface` and capture thread.
2. `libresource_mgr_client`'s `ResourceManagerClientInit` calls
   `g_main_context_new` at `0x69ba` and `g_main_loop_new` at `0x69c4`.
3. `libums_connector_impl` additionally refs that loop/context at `0x875e` and
   `0x876a`, then calls `LSRegisterPubPriv` at `0x87a8`.
4. Failed registration branches to `0x96e4`, constructs a runtime error and
   throws at `0x977e`. The caller's exception cleanup does not release the
   context/loop.

A constructor-only harness reproduced three failures: each logged a new context
and loop from those resource-manager call sites, **no corresponding unrefs**,
and one additional FD (total 5 → 6 → 7 → 8). With a valid temporary role and
50 ms between create/release, three cycles returned to five FDs; tracing showed
the corresponding unrefs. These harness totals include standard descriptors and
the capture lock, unlike the table below. Heap retention was not quantified.

There is no returned partial object to pass to `vtCapture_release`. Closing the
observed eventfd would leave GLib objects/refcounts and possibly other native state
unrepaired. `vtCapture_createEx` also calls the same `VtDriver` constructor at
`0x19c34`; it does not avoid this ownership path.

A **preventive application-side check is possible**: register/unregister unique
names under both allowed Luna prefixes before invoking the constructor, freeing
`LSError` on every path. A temporary harness used ordinary `LSRegister` and
`LSUnregister`, without allocating a GLib loop. Twenty permission-denied attempts
kept five FDs and never called the constructor. With the temporary role present,
three prechecked create/release cycles succeeded and returned to five FDs.
This checks name-registration permission at that instant, not outbound-service
permission, native initialization success, or absence of a subsequent bus failure.
It is a tested prototype, **not integrated into production**. After an actual
constructor exception, stopping retries/recycling the owning process is safer
than attempting undocumented cleanup.

Normal completed video teardown releases its capture resources:

- `stopVtCapture` (`0x118f0`) stops streaming.
- `postprocessVtCapture` (`0x10c00`) invokes stop, clears mappings and requests
  zero buffers. `clearBuffer` calls `munmap`.
- `finalizeVtCapture` (`0xf7b0`) closes the device and releases the resource-manager
  allocation.
- `vtCapture_release` destroys the driver and its internal threads/LS2 interface.
  Thread joins are ordinary `pthread_join`, without a timeout. The stream-stop
  path has a two-second condition wait, which does not bound all teardown calls.
- On this firmware, preprocessing itself starts streaming; the public process
  entry point is not the only start boundary. Keep partial-failure cleanup guards.

### Video release has a reproduced lost-wakeup race

This is now a reproduced userspace hang, not merely an observation that joins
lack timeouts. The relevant instructions in `libvtcapture` are:

- Capture thread: at `0x1152a`, broadcast the stopped condition (`driver + 936`);
  at `0x11536`, wait on the wakeup condition (`driver + 864`). There is no
  intervening shutdown-predicate check. The loop's lifetime flag is checked later,
  at `0x11558`, after this wait and dequeue work.
- Destructor: clear the lifetime flag at `0x10760`, signal the wakeup condition
  at `0x10762`, then join the capture thread at `0x10768`.
- `Thread::join` reaches ordinary `pthread_join`. The destructor does not repeat
  the wakeup or impose a deadline.

A scheduler can therefore interleave the operations as follows:

```text
Capture thread                    Thread releasing the driver
broadcast stopped
                                  clear lifetime flag
                                  signal wakeup (no waiter yet)
                                  pthread_join ───────────────┐
pthread_cond_wait (no future signal) ─────────────────────────┘
```

A temporary process-local tracing shim delayed only the second capture-thread
`pthread_cond_wait` by 500 ms, after the stopped broadcast and before calling the
real wait. The **unchanged production Rust wrapper**, via `cleanup_probe`, captured
video and reached drop. Its destructor signalled during the delay and entered
join. After the delay ended, both threads remained in futex waits until the
external eight-second watchdog killed only the probe. Native thread IDs and
condition addresses match between signal, wait and join. The same two-cycle
tracing control without a delay completed normally; capture-thread joins took
about 16 ms.

A constructor-only create → 50 ms → release harness reproduced the same hang by
delaying its first wait. This removes our stop/postprocess/finalize calls from the
reproduction, ruling out duplicate cleanup as a necessary cause. Finally,
**immediate create → release without any preload, tracing or injected delay hung
on the first cycle** of a planned 50-cycle run. `/proc` showed the main thread
waiting to join the native capture thread, that thread asleep in a condition
futex, and the two Luna threads polling. This early-release control did not open a
capture stream; its relevance is startup-abort cleanup. The delayed production
probe separately establishes the race after actual capture.

The watchdog successfully reaped each hung probe; no task remained in
uninterruptible device sleep. There is no evidence here of a kernel-driver hang.
A final uninstrumented production-wrapper control completed three video cycles.

Changing the Rust drop order cannot remove the race inside `vtCapture_release`.
A fixed sleep after stop only changes scheduling probability. The exposed C API
has no capture-thread shutdown acknowledgement or bounded-release operation.
Skipping release, detaching the thread, or timing out while freeing the driver
would retain live native state or risk use-after-free. Interposing private native
synchronization would be a firmware-specific patch, not a portable ownership fix.
Keeping a driver alive across sessions avoids some releases but leaves final
shutdown and failure recovery unresolved. No such workaround was added.

Other native joins (resource-manager and LS2 threads) remain unbounded; their
failure modes were not separately induced. The LS2 loop's running-check/quit/join
sequence is a separate review concern, not a second confirmed race in this report.

`libdile_vt` on this TV is a stub: Create/CreateEx return null, and Start/Stop
return `-1`. GM's supported library names are absent. Their successful hardware
lifecycles were not tested; existing fake-backend tests still apply.

## Measurements

### Ghidra and Frida follow-up: constructor registry and driver reuse

Ghidra 12.2 analyzed the video, resource-manager and connector implementation
libraries. It selected `ARM:LE:32:v8`, default compiler, image base `0x10000`.
Addresses in this report are ELF-relative: subtract `0x10000` from the exported
Ghidra addresses. Important exception paths were checked against ARM disassembly;
decompiler output alone does not reliably reconstruct C++ unwind cleanup.

**Failed construction also leaves a dangling registry entry.** The video
library's `ResourceManagerClient` wrapper registers itself at `0x15916` before
calling `init` at `0x15920`. Its exception path at `0x1592a–0x15956` destroys
members without unregistering; outer constructor cleanup frees the wrapper.
The global `g_rmcs` list is at `0x312c8`. `policyAction` at `0x16c68` later walks
this list and reads fields through its object pointers.

Frida Gum hooks confirmed three denied constructors each registered an object,
freed that same address, and never called `unregisterRmcInstance`. After each
exception, a read-only walk found the freed address still in the registry.
List sizes were 1, 2, 2: allocator address reuse explains why this need not grow
by one every time. The successful control, using only the two registry hooks,
completed three create/release cycles and returned the list to size zero each
time. A subsequent policy callback dereferencing a stale entry is a **static
risk**, not a dynamically induced use-after-free in these tests. This strengthens
the recommendation to stop retrying after an actual constructor exception.

The compatible tooling came from the
[openlgtv 2026.08 SDK](https://github.com/openlgtv/buildroot-nc4/releases/tag/2026.08-webos),
whose [Frida Gum recipe](https://github.com/openlgtv/buildroot-nc4/blob/2026.08-webos/package/libfrida-gum/libfrida-gum.mk)
pins commit `0c8df6742d2c7a08c32fc3c3b691779b062fb34a`. The downloaded
`arm-lgtv-linux-gnueabi_sdk-buildroot-x86_64.tar.gz` matched SHA-256
`a8d82460b3a96a96cb2f3847df19d2c641672a9d8facf7141502c29dd86f9000`.
Its ARM32 little-endian soft-float Gum archive and dependencies were linked into
our temporary probe using the existing Mac cross-compiler, with archive symbols
hidden to avoid replacing the vendor's GLib. A local function-hook assertion
passed on the TV before the vendor hooks ran. No Frida server, Gadget, input-hook
application, SDK executable or system library was installed on the TV.

Instrumentation limits: `operator delete` was too short for Gum to attach, so
the denied-constructor probe observed `free` instead. A valid-role control with
the broad `free` hook timed out; its cause was not established and is not counted
as another confirmed vendor race. Removing that hook produced the successful
registry control above. Instrumentation can change scheduling and runtime behavior.

**One driver can serve multiple capture sessions through the public API.** A
temporary probe created it once, then repeated init → preprocess → process →
frame metadata → stop → postprocess → finalize, alternating 320×180 and 160×90.
The final eight-cycle control asserted reported plane geometry and non-null,
positive-sized frame buffers. Every cycle succeeded. Idle totals remained ten
FDs and four threads; maps settled at 121. No capture-device FD remained between
sessions. Final release returned to five FDs and one thread. These totals include
standard descriptors and the capture lock; they differ from the older table's
counting convention. No pixel contents were inspected or logged.

However, driver reuse shares the lost-wakeup problem. Delaying the native
thread's second entry into its wait by 500 ms let cycle two's start signal arrive
before the wait. Init, preprocess, process and plane-info all returned success,
but no frame became ready within the probe's 100 polls at 10 ms intervals. No
destructor had run. `processVtCapture` at `0xedb0` only validates the client and
returns; repeating it does not wake the thread. `streamOnOff(true)` also returns
without signaling when its streaming flag is already true.

A second probe allowed **one** retry after the frame deadline, using the normal
stop → postprocess → finalize → init sequence on the same driver. It recovered
cycle two, captured cycle three, and completed final release. The next init began
about 4.56 seconds after the missed start signal. This establishes a possible
recovery path for this induced startup stall, not bounded cleanup or general
failure recovery. Native calls still require a watchdog outside the calling thread.

Other proposed shortcuts were ruled out by the call paths:

- `createEx` supplies a resource-policy callback, not a shutdown acknowledgement.
  `initEx` changes resource-manager priority, not thread ownership.
- `fdClose` only closes the descriptor. Closing before the capture thread stops
  can race with dequeue; it is not a synchronization primitive.
- `postprocess` performs the actual buffer unmaps. The destructor deletes buffer
  containers; skipping postprocess is not equivalent cleanup.
- Replacing the wait with periodic timeouts would let the capture thread proceed
  into dequeue while stopped unless its surrounding state machine were also fixed.

Persistent ownership plus a retry is therefore a mitigation worth considering,
but it does not remove the need for process-level recovery after constructor
failure or a release deadline. No production lifecycle changes were made in this
follow-up. Probes, logs, hashes and the evidence chain are archived in this research folder.
Vendor Ghidra exports remain outside the repository; their original paths and
hashes are listed in `evidence/external-artifacts.json`. Temporary TV probe directories and manifests were
absent after testing, and the installed Prysm supervisor remained present.

### Earlier cycle measurements

All captures used the existing active input, 320×180 and the default 30 FPS.
FD counts below exclude stdin/stdout/stderr and the shared capture lock.
The logging-lock FD is included where present.

| Run | Cycles | After-drop FDs | Result |
| --- | ---: | --- | --- |
| Original HAL, one frame | 8 | 5 → 40 | Reproduced growth |
| Original HAL, no frames | 3 | 5 → 15 | Acquisition not required |
| Original HAL, library held loaded | 20 | 5 throughout | No continuing growth |
| HAL lifetime fix | 50 | 5 throughout | All cycles captured successfully |
| Video only, valid temporary role | 8 | 1 throughout | 72 frames, no capture-device FDs after drop |
| Combined, HAL fix | 20 | 6 throughout | 40 video frames; UI captured every iteration |
| Video create failure, no role | 8 | 2 → 9 | One new eventfd per failure |
| Same failure, video library held loaded | 8 | 2 → 9 | Lifetime fix does not address this leak |

In the 50-cycle HAL test, after-drop mappings stayed at 77 and RSS settled at
3160 KiB. Combined mappings stayed at 145 and RSS settled at 4836 KiB. These
measurements precede the signal-waiter addition (which adds a thread/stack).
They are short restart tests, not a long-run memory or image-quality guarantee.

## Architecture decision

The original five-handle observation alone no longer justifies rejecting a
dedicated capture thread: its growth has an application-side library-lifetime fix.
Normal video restarts also worked. A future threaded implementation must keep all
capture operations on one long-lived thread and block shutdown signals before
creating that thread and the native threads it starts.

The deeper investigation supports retaining the current process boundary:
constructor exceptions leak and leave stale registry pointers before returning a
handle, and native release has a demonstrated lost-wakeup hang even without
injected delays. Driver reuse and one public-API retry recovered an induced start
stall, but do not repair failed construction or bound final release. A thread-only daemon would need to
restart the **whole service process**, under external supervision, after such a
failure or a missed shutdown deadline. Replacing just the capture thread does not
reclaim process resources. Neither approach can force a task out of uninterruptible
kernel sleep.

The current capture loop also retries unavailable video within a session: process
isolation reclaims these constructor-failure leaks only when the worker exits.
Before unattended use, prevent known permission failures with a registration
precheck and classify actual constructor exceptions as unrecoverable in that
process, rather than retrying them indefinitely. The precheck prototype only
prevents the demonstrated denial case; it does not reclaim failed construction
or bound release. These production policy changes and the unfinished Rust
supervisor port were not implemented or deployed during this investigation.

A dedicated capture thread remains an architectural option **if whole-service
termination/restart is the accepted recovery mechanism**, with a watchdog outside
the blocked capture thread and external service supervision. It is not a safe way
to restart only capture after the demonstrated hang. This investigation changes
neither the supervisor implementation nor the deployment architecture.

## Reproduction and safety

Local validation: 40 worker tests and four probe tests passed; capture Clippy
passed with `--all-targets -- -D warnings`. Both the worker and diagnostic example
built with the soft-float webOS SDK. The final worker binary was not installed on
the TV. The partial `webos-service` port was preserved and was not validated here.

[`cleanup_probe.rs`](../../../crates/capture/webos-capture/examples/cleanup_probe.rs) reuses production modules without adding a public
capture library API. It logs `/proc/self` descriptor targets, mappings, thread
wait states and RSS at lifecycle boundaries. It never outputs frame contents.
It bounds cycles/frames, refuses force-capture and holds `/tmp/prysm-capture.lock`.
It continues through initialization errors to measure repeated failure, then exits
nonzero if any cycle failed. Fake/native unit tests run before live testing.

Use the environment from `build-webos.sh`, adding `--example cleanup_probe` to its
Cargo build command. Upload only the resulting example to a unique temporary
directory. Check current power/capture state first; do not stop the user's session.
Run under an external bound, for example:

```sh
timeout -s TERM -k 3 20 /tmp/UNIQUE-PROBE-DIR/probe \
  --source ui --ui-backend halgal --cycles 50 --frames 1
```

Video success tests additionally used a temporary role with the existing capture
role's allowed names/outbound services, restricted to the probe's exact executable
path, registered by its own volatile manifest. The installed role and manual
overlay were not edited. Remove the probe's role/manifest and scan services again
after testing. Capture-lock contention must abort the probe, not stop the installed
worker. A bounded timeout is a safety limit, not proof of bounded native shutdown.

No system partitions, vendor binaries, settings, autostart, power, inputs, or
installed Prysm artifacts were changed. Temporary probe binaries and registration
files were removed. Binary copies for disassembly were explicitly authorized and
kept only in local `/tmp/prysm-cleanup-evidence`, outside the repository.

Follow-up diagnostic source, runner and raw traces are archived in this folder
under `scripts/` and `evidence/`; earlier runs are in `evidence/earlier/`.
No tracing/interposition code is linked into production. The runner uses a unique
volatile role/manifest, an external eight-second watchdog, and `finally` cleanup.
Its named evidence files are `constructor-denied`, `constructor-valid`,
`teardown-control`, `teardown-delayed-wait`, `release-only-delayed`,
`release-only-no-delay`, `preflight-denied`, `preflight-valid` and
`final-video-control` (`.stdout` / `.stderr`). The shim's wait-call offset applies
only to the hashed firmware binary below. Inspect it before reuse on another TV.
All temporary TV executable, tracing and registration files were removed.

The precheck uses the documented
[LSRegister/LSUnregister API](https://www.webosose.org/docs/reference/luna-service2-library/luna-service2-library-api-reference/);
its behavior and descriptor counts above were measured on this retail TV, rather
than inferred from OSE documentation.

SHA-256 of examined TV binaries:

```text
libhalgal.so.1.0.0     2ae4d20dc5759aa9333cd09a15b02a9cd39aecf3c573e3fb8341f8ea395d29cb
libkadaptor.so         ea6d8b4d13c5e8d2f260178f0f47ba200f57fbf808d471a59369dbfd10d3c170
libvtcapture.so.1.0.0  1e16249a0b7d3b87e35d4fd75b89038bddc0300845c9cb96ad23a344099a3e6b
libdile_vt.so.0.1.0    25f76016ca775682b475ae2489145521a9054754db4beb0514fc92a951c2fb2b
libresource_mgr_client.so.1.0.0 d6cdcb0292043e7bbdaa1271a7b38a9db723d51348cd732c4e22ede02b8377a6
libums_connector.so.1.0.0       9088a19ceb4432213bcf57de571a88b9934d047d7c9aa6580d43784214f66d63
libums_connector_impl.so.1      03efd4af3f3c5598d84c8f2a8d1bd3771b15d54db180b09a0cc8e1e185470b19
```

Primary upstream comparison: [HAL backend](https://github.com/webosbrew/hyperion-webos/blob/00c932f092084b234b3f619b060a98fa134c12bd/unicapture/backends/libhalgal.c),
[video backend](https://github.com/webosbrew/hyperion-webos/blob/00c932f092084b234b3f619b060a98fa134c12bd/unicapture/backends/libvtcapture.cpp).
Their lifecycle calls informed the comparison; measured behavior and disassembly
of this TV determine the conclusions above.
