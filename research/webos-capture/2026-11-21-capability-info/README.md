# What `vtCapture_capabilityInfo` reports — 2026-11-21

**Status:** desk research and a probe that is ready to run. Nothing has run on
a TV yet, and no value in this note was measured. Issue `prysm-884`. The
maintained guide is [TV lifecycle and permissions](../../../docs/developers/webos.md).

## Finding

No public source decodes LG's `_LibVtCaptureCapabilityInfo`. The only public
declaration of it is a placeholder. LG's kernel capture interface does publish a
capability struct, though, and the evidence suggests the library copies that
struct.

- The kernel struct is `struct v4l2_ext_capture_capability_info`, read through
  the `V4L2_CID_EXT_CAPTURE_CAPABILITY_INFO` control. It is 40 bytes: a flags
  word, scale-up and scale-down limits, a frame buffer count, a maximum
  resolution, a plane count and a pixel format. It has **no frame rate**.
- LG's library already mirrors the kernel's plane info byte for byte, in the
  layout Prysm uses on every frame. That makes "the library copies the kernel
  struct" (layout A below) the leading candidate. It is still unconfirmed.

[`vt_capability_probe`](../../../crates/adapters/capture-webos-sys/examples/vt_capability_probe.rs)
settles the question in one run of about a second. It does not start capture
and leaves no files behind. [`run-tv.sh`](run-tv.sh) wraps it in a bounded
session.

## Public sources

| Source | What it says | Trust |
| --- | --- | --- |
| [tv-native-apis `vtCaptureApi_c.h`](https://github.com/webosbrew/tv-native-apis/blob/052b449aceabe3aa0eb5e90d02f3b11b4608a000/libvtcapture/include/vtcapture/vtCaptureApi_c.h#L45-L58) (MIT; Prysm's [`native/vtCaptureApi_c.h`](../../../crates/adapters/capture-webos-sys/native/vtCaptureApi_c.h) is a copy) | Declares the struct as `VT_DRIVER *driver` followed by `dummy1`…`dummy10`: one `int32_t *` and nine `int16_t *` pointers. It carries one comment, `//0x14`, at `dummy5`, and declares the call as `(driver, clientid, struct *)`. | **Low** for the field layout: placeholder names from reverse engineering. The [file history](https://github.com/webosbrew/tv-native-apis/commits/052b449aceabe3aa0eb5e90d02f3b11b4608a000/libvtcapture/include/vtcapture/vtCaptureApi_c.h) (May–June 2021) never fills it in. **High** for the call's signature, which matches the C++ symbol below. On armv7 it declares 44 bytes. |
| [tv-native-apis `samples/vtcapture/capture/capture.c`](https://github.com/webosbrew/tv-native-apis/blob/052b449aceabe3aa0eb5e90d02f3b11b4608a000/samples/vtcapture/capture/capture.c) | Calls create, init, preprocess, planeInfo, process and so on, but **not** `vtCapture_capabilityInfo`. | — |
| [hyperion-webos `unicapture/backends/libvtcapture.cpp`](https://github.com/webosbrew/hyperion-webos/blob/00c932f092084b234b3f619b060a98fa134c12bd/unicapture/backends/libvtcapture.cpp) (MIT) | Does **not** call `vtCapture_capabilityInfo`. It sets `buf_cnt = 3` and a frame rate of 60 when none is configured. Its dump enum is `SCALER_INPUT, SCALER_OUTPUT, DISPLAY_OUTPUT, BLENDED_OUTPUT, OSD_OUTPUT, HISTOGRAM_OUTPUT` (0–5). [PicCap](https://github.com/TBSniller/piccap) has no native code of its own; it bundles hyperion-webos as a git submodule (its `.gitmodules`). | High for the call sequence, which Prysm follows. |
| LG's v4l2-ext capture header, as carried in Amlogic kernel trees (GPL-2.0+ OR MIT): [CoreELEC `videodev2-ext.h` L2519–L2558](https://github.com/CoreELEC/common_drivers/blob/0af3d219abcefde37a0313286e6a5b71d1e5cee7/include/linux/amlogic/media/v4l2-ext/videodev2-ext.h#L2519-L2558), [control ids](https://github.com/thirdreality/voice-music-assistant/blob/0af62fc8445bfbf3873bc32696b79c4394b0e48d/sources/kernel/include/uapi/linux/v4l2-ext/v4l2-controls-ext.h#L595-L616) | `struct v4l2_ext_capture_capability_info` has the fields listed under layout A below. It also has the flag bits (`INPUT_VIDEO_DEINTERLACE 0x1`, `DISPLAY_VIDEO_DEINTERLACE 0x2`, `SCALE_UP 0x4`, `SCALE_DOWN 0x8`, `DIVIDE_FRAMERATE 0x10`, `VIDEOTOVIDEO 0x20`), the pixel format enum (0 YUV420 planar, **1 YUV420 semi-planar**, 2 YUV420 interleaved, 3–5 YUV422, 6–8 YUV444, 9 RGB, 10 ARGB) and the plane count enum (1 interleaved, **2 semi-planar**, 3 planar). The capture location enum is `SCALER_INPUT 0, SCALER_OUTPUT 1, DISPLAY_OUTPUT 2, BLENDED_OUTPUT 3, OSD_OUTPUT 4`, the same values as Prysm's dump modes 2 and 3. The capture device is `V4L2_EXT_DEV_NO_CAPTURE 60`. Frame rate has its own controls (`…_OUTPUT_FRAMERATE`, `…_DIVIDE_FRAMERATE`). | **High** that this is LG's kernel interface: LG's name prefix, "LGE chips only" comments, and the same enums as libvtcapture. **Medium** that the TV fills it the same way: the measured TV is a Realtek SoC, not Amlogic. |
| [Amlogic `vdin_v4l2_if.c` L889–L900, L2155–L2175](https://github.com/thirdreality/voice-music-assistant/blob/0af62fc8445bfbf3873bc32696b79c4394b0e48d/sources/kernel/drivers/amlogic/media/vin/tvin/vdin/vdin_v4l2_if.c#L2155-L2175) | An Amlogic driver's defaults. The flags are scale-down, divide-framerate and both deinterlace bits. Scale-up is 3840×2160, scale-down 360×240, 5 frame buffers, maximum resolution 0,0 3840×2160, 1 plane, YUV422 interleaved. The control copies exactly `sizeof` the struct, so the struct is the whole payload. | Shows what the fields mean. The values belong to another SoC. |
| [ishiishikou/lg-webos-tv investigation log](https://github.com/ishiishikou/lg-webos-tv/blob/ca8486d1e7ef27d0c2a4e80e9f4b5b41bb30b927/docs/investigation-log.md) | `libvtcapture.so` contains the string `/dev/video60`. The log lists the `V4L2_CID_EXT_CAPTURE_*` names. It records no values: `vtCapture_create` failed there at `LSRegisterPubPriv`. | Medium: it ties the library to the kernel device above. |
| [webosbrew dev-toolbox-cli symbol data](https://github.com/webosbrew/dev-toolbox-cli/blob/2d5a7b6881850d825aa7fdc6c0b5e53c91dc40d3/common/data/03.40.82.01-HE_DTV_W21P_AFADATAA/libvtcapture.so.1.0.0.json) | Symbol lists of `libvtcapture.so.1` (package `screen-capture-webos`) from 7 firmware images, named by firmware version and platform code (W20L, W21P, W22O, W22H, W23O). All 7 contain `vtCapture_capabilityInfo`, `VtDriver::vtCapabilityInfo` and `VtDriver::getDeviceCapabilityInfo`. | High that the call exists across those firmware generations. Nothing about the layout. Which webOS version each code means was not checked. |
| [adeze/lg-hue-sync `src/capture/vtcapture.rs`](https://github.com/adeze/lg-hue-sync/blob/cc3e50f9cfe419e6b90efa76f0e473f108f18239/src/capture/vtcapture.rs) | Another Rust vtCapture client (Hue sync). It declares the same properties layout and `buf_cnt: 3`. It does **not** call `vtCapture_capabilityInfo`. | — |
| [openlgtv webOS hacking notes (gist)](https://gist.github.com/Informatic/1983f2e501444cf1cbd182e50820d6c1); [LG developer forum: "How to Capture Screen on Webos TV"](https://forum.webostv.developer.lge.com/t/how-to-capture-screen-on-webos-tv/7880) | The gist covers only the Luna `executeOneShot` capture, not libvtcapture. On the forum, LG staff say no screen capture API is provided. | — |
| webOS OSE (`github.com/webosose`) | A code search finds no `vtcapture` or `LibVtCapture`: libvtcapture is not open source. | — |

The earlier lifecycle work made a symbol dump of the measured TV's library
([E-001 exports](../2026-11-01-lifecycle/evidence/E-001-vt-exports.json)).
These facts come from that dump, not from a new measurement:

- `vtCapture_capabilityInfo` is 280 bytes of code. It forwards to
  `VtDriver::vtCapabilityInfo(const std::string&, _LibVtCaptureCapabilityInfo&)`,
  268 bytes, which takes the struct **by reference**, as `vtPlaneInfo` takes
  `_LibVtCapturePlaneInfo&`.
- `VtDriver::getDeviceCapabilityInfo()` (580 bytes, no arguments) presumably
  reads the kernel control into the driver object.
- Next to it are `getCaptureFrameRate(unsigned*)`, `setCaptureFrameRate(unsigned)`
  and `getCapturePlanePropertes(v4l2_ext_capture_plane_prop*)`. The library
  uses the v4l2-ext types by name, and keeps frame rate apart from capability.

**Searched:** GitHub code search for `vtCapture_capabilityInfo`,
`_LibVtCaptureCapabilityInfo`, `v4l2_ext_capture_capability_info` and
`V4L2_CID_EXT_CAPTURE`, plus the webosose organisation. (GitHub code search
skips most forks.) Also the repositories above, and two web searches, for the
call name and for openlgtv or forum threads. None of them prints a dump of
this struct or calls it, for this TV or any other. Forks of hyperion-webos and
PicCap, Discord and Telegram discussions, and LG's opensource.lge.com tarballs
were **not** searched.

### Why layout A leads

The library's other structs, set against the kernel's:

| Library struct (as Prysm uses it) | Kernel struct | Same bytes? |
| --- | --- | --- |
| `_LibVtCapturePlaneInfo`: `int32 stride`, `int16 ×4` plane region, `int16 ×4` active region (20 bytes) | `v4l2_ext_capture_plane_info`: `u32 stride`, `u16 ×4`, `u16 ×4` (20 bytes) | **Yes.** Prysm checks the stride and region against them on every open. |
| `_LibVtCaptureProperties`: `dump`, `x,y`, `w,h`, `buf_cnt`, `frm` (20 bytes) | `v4l2_ext_capture_plane_prop`: location, `u16 ×4` rect, `buf_count` (16 bytes) | Its first 16 bytes match; the library adds the frame rate at the end. |

So the library copies kernel structs, sometimes with a field added. The
placeholder's `//0x14` comment falls on the fifth 32-bit word in either
reading, which says nothing.

## What Prysm does now

- **Declaration:** [`native/vtCaptureApi_c.h:45-58`](../../../crates/adapters/capture-webos-sys/native/vtCaptureApi_c.h#L45-L58)
  (struct) and [`:79`](../../../crates/adapters/capture-webos-sys/native/vtCaptureApi_c.h#L79)
  (call). There is no Rust binding: [`ffi.rs`](../../../crates/adapters/capture-webos-sys/src/ffi.rs)
  has no capability type, and the `VtApi` table
  ([`vtcapture.rs:96-107`](../../../crates/adapters/capture-webos-sys/src/vtcapture.rs#L96-L107))
  never loads the symbol. The ABI layout test ([`native/abi.cpp`](../../../crates/adapters/capture-webos-sys/native/abi.cpp))
  covers properties, plane info and buffer info only.
- **The one value:** `MODE` in [`capture-webos/src/settings.rs:5-16`](../../../crates/adapters/capture-webos/src/settings.rs#L5-L16)
  is width 2–1920 step 2, height 2–1080 step 2, fps 1–60 step 1. Its doc
  comment already names `vtCapture_capabilityInfo` as the source it should
  come from.
- **`modes`:** [`capture-webos/src/lib.rs:82-85`](../../../crates/adapters/capture-webos/src/lib.rs#L82-L85)
  returns `MODE.into()` whatever the settings. `WebosSettings::validate`
  ([`settings.rs:67-69`](../../../crates/adapters/capture-webos/src/settings.rs#L67-L69))
  checks against the same value.
- **The same limits, repeated elsewhere:** the settings schema bounds
  ([`settings.rs:31-44`](../../../crates/adapters/capture-webos/src/settings.rs#L31-L44),
  tested in `schema_matches_serde_shape`) and `Options` docs
  ([`vtcapture.rs:15-20`](../../../crates/adapters/capture-webos-sys/src/vtcapture.rs#L15-L20)).
  The plane region check in `prepare` allows 2–1920 × 2–1080 and a stride up
  to 32768 ([`vtcapture.rs:358-368`](../../../crates/adapters/capture-webos-sys/src/vtcapture.rs#L358-L368)),
  `copy_plane` allows at most 1080 rows
  ([`vtcapture.rs:193`](../../../crates/adapters/capture-webos-sys/src/vtcapture.rs#L193)),
  and the buffer count is fixed at 3 ([`vtcapture.rs:284`](../../../crates/adapters/capture-webos-sys/src/vtcapture.rs#L284)).
- **What a decoded struct could replace:** the 1920 and 1080 upper bounds (from
  the maximum resolution or the scale limits), a lower bound if the scale-down
  limit turns out to mean a minimum output size, and the fixed 3 buffers
  (capped by the reported count). The fps span stays: the struct holds no rate
  if layout A holds. The UI tap bounds (256–320 × 144–180, `settings.rs:17-20`)
  belong to HAL GAL, not to this struct.

**Earlier measurements to compare with.** Every measurement comes from one TV:
an LG RTD2875Q on webOS TV 10.3.2, showing a 3840×2160 panel, according to the
[UI capture note](../2026-11-12-ui-capture/README.md#measurements). The VT tap
has delivered 320×180, 640×280, 640×360 and 1280×720
([on-TV pipeline](../2026-11-04-on-tv-pipeline/README.md): 29.97 fps at
1280×720), always at 30 fps with 3 buffers. **1920×1080, rates above 30 fps,
sizes below 320×180 and other buffer counts were never measured.** The
`MODE` comment says as much. No earlier dump of this struct exists in the
repository.

## Candidate layouts

Offsets are in bytes on armv7, little-endian.

**A — the kernel struct, copied as is (40 bytes).**

| Offset | Field | Type |
| --- | --- | --- |
| 0 | `flags` | u32, bits 0x01–0x20 as above |
| 4, 8 | `scale_up_limit_w`, `_h` | u32 |
| 12, 16 | `scale_down_limit_w`, `_h` | u32 |
| 20 | `num_video_frame_buffer` | u32 |
| 24 | `max_res` | u16 x, y, w, h |
| 32 | `num_plane` | u32 enum (1–3) |
| 36 | `pixel_format` | u32 enum (0–10) |

**A′ — A with a field added**, as `_LibVtCaptureProperties` adds `frm`. A
frame rate or a dump location could sit at offset 0 or 40, giving 44 bytes:
the placeholder's size.

**C — a compact library form** with `int16` pairs (`VT_RESOLUTION_T`)
instead of u32 limits. Its words would hold two plausible sizes each: for
example, 1920 in the low half and 1080 in the high half of a single word.

**B — the placeholder taken literally:** eleven pointers that the library
writes through. This is unlikely. The C++ side takes a reference to a plain
struct, and the caller would have to supply ten valid `int16_t` pointers, which
no known caller does.

## The probe

[`crates/adapters/capture-webos-sys/examples/vt_capability_probe.rs`](../../../crates/adapters/capture-webos-sys/examples/vt_capability_probe.rs)

- **Buffer.** It hands the library a 256-byte body, with 64 guard bytes on
  each side. Every body word holds the address of its own 64-byte scratch
  block, which is filled with `0x5A`. So:
  - a changed word is a value the library wrote;
  - a changed scratch block is a pointer the library wrote through (layout B),
    and dereferencing one does not crash;
  - the last changed word gives the size written;
  - a changed guard word is an out-of-bounds write.
  - A written byte equal to the address byte it replaced is invisible. The
    extent is therefore rounded up to whole words, and a zero written over an
    address's zero high half looks untouched.
- **Output.** It prints a hex dump marking changed bytes, then every word as
  hex, u32, i32 and both 16-bit halves (u16 and i16), with its state. It ends
  with a one-line decode under layout A.
- **Sequence.** It mirrors `VtCapture` and adds nothing to the TV:
  1. Take `/tmp/prysm-capture.lock` with the worker's flags (`O_NOFOLLOW`,
     0600, `try_lock`). It refuses while a worker holds the lock and never
     deletes the file.
  2. Run the same Luna registration precheck as the worker.
  3. Load `libvtcapture` (never unloaded).
  4. Call `vtCapture_create`, then `vtCapture_init` with the same caller id
     and `"00"` client, then **`vtCapture_capabilityInfo`**.
  5. With `--preprocess` only: `vtCapture_preprocess` at 640×360, 30 fps,
     dump 2, 3 buffers, then the call again. Use this if the call fails or
     writes nothing after init alone, in case the library fills the struct
     only once a plane is configured.
  6. `vtCapture_process` is **never called, so capture never starts**.
  7. Release by stage reached, in `VtCapture::cleanup`'s order: postprocess
     (if preprocessed), finalize (if initialized), release. There is no stop,
     because capture never ran.
- **What create and init do.** They are LG's normal session setup, which
  Prysm runs on every open. Per the [lifecycle report](../2026-11-01-lifecycle/report/2026-11-01_reverse-vt-lifecycle-report.md),
  create registers a resource manager client before init. Finalize and
  release are the same teardown Prysm uses.
- **Bounds.**
  - All native calls run on the main thread, with SIGINT and SIGTERM blocked,
    as `block_signals` does for Prysm's capture thread.
  - A signal thread turns SIGINT or SIGTERM into "release after the current
    call".
  - A watchdog matches `Limits::TV`: 5 s per call, 4 s for release. A call
    that overruns ends the process with exit 3 and prints the call it hung
    in. Killing the process is how the supervisor recovers a hung worker
    too.
  - Every call is logged before it starts.
- **Exit codes:** 0 dump printed, 1 setup or native failure, 2 not a TV, not
  root or bad arguments, 3 watchdog, 4 dump printed but a call returned
  non-zero.
- **Host tests.** `cargo test -p prysm-capture-webos-sys --example
  vt_capability_probe` drives the report through the real C++ boundary with a
  fake library. A plain `cargo test` builds examples but does not run their
  tests.
- **Exception boundary.** The call goes through the crate's existing C++
  boundary (`prysm_vt_plane`), which has the same driver, client, struct
  shape. No production code changed.

### Building and running

The probe needs a Luna role for its own path. VTCapture's registration is tied
to the executable's path ([UI capture note, limits](../2026-11-12-ui-capture/README.md#background-ui-thread-next-to-a-simulated-30-fps-video-loop)).
`run-tv.sh` stages one, the way `packaging/webos/tests/native-smoke.py
--capture` does. Prysm's own `setup` uses the same role-only manifest, but the
staging is **unverified on this TV**, since TV acceptance has not run. If Luna rejects it, the probe stops at the precheck (exit 1) before
any vendor call.

```sh
# Development host, with the webOS SDK
WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot . packaging/webos/env.sh
cargo build --target armv7-unknown-linux-gnueabi --profile webos \
  -p prysm-capture-webos-sys --example vt_capability_probe

# Turn lighting off (full) or capture off (slim) in the TV app, then:
PRYSM_TV=root@<tv-address> research/webos-capture/2026-11-21-capability-info/run-tv.sh \
  target/armv7-unknown-linux-gnueabi/webos/examples/vt_capability_probe \
  <local-evidence-dir>
# Only if the call failed or wrote nothing:
PRYSM_TV=root@<tv-address> research/webos-capture/2026-11-21-capability-info/run-tv.sh \
  target/armv7-unknown-linux-gnueabi/webos/examples/vt_capability_probe \
  <local-evidence-dir>-pre --preprocess
```

`run-tv.sh` does the following:

- It refuses while an earlier probe still runs, or while its manifest name
  already exists. It does not refuse while Prysm runs: the full worker and the
  supervisor stay up while the TV is on. The capture lock is the exclusion.
  With lighting off (full) or capture off (slim), Prysm closes its capture
  source and releases the lock. If Prysm still holds it, the probe stops at
  once (exit 1, "a Prysm capturer holds…"), before any vendor call.
- It works in a private `/tmp/prysm-capability.XXXXXX` directory and writes
  files without overwriting.
- It writes one manifest, `/var/luna-service2/manifests.d/io.prysm.tv.capprobe.json`,
  pointing at a copy of the packaged worker role with the probe's path. Then
  it rescans Luna. The manifest has the same keys (`id`, `version`,
  `roleFiles`) as the worker manifest that `setup` writes
  ([`install.rs` `worker_manifest`](../../../crates/hosts/webos-service/src/install.rs)).
- It records `videooutput` status, `uname` and the SoC string. It runs the
  probe under `timeout -s TERM 30`, then saves the `dmesg` tail.
- In an exit trap it cleans up:
  - it sends TERM to any leftover probe, found by its exact `/proc/*/exe`
    path, and waits up to 25 s (never KILL);
  - it removes the manifest only if it is still byte-identical, and rescans;
  - it removes the directory.

  If anything is left over, it stops and says what it left.

The `<local-evidence-dir>` contents belong in an `evidence/` folder here once
a run has happened. Before committing, strip addresses and serial numbers from
`platform.txt` and `dmesg-tail.txt`. Have the TV on a known HDMI input, and
note its signal (resolution and rate). The report may depend on the source as
well as the SoC.

Afterwards, re-enable Prysm and check that it reports `session: running`. A
failed release would show up there and in the `dmesg` tail.

## Decode plan

Read the probe's `written extent`, guard and pointer lines first.

| Observation | Means |
| --- | --- |
| Stops at the Luna precheck (exit 1, "Luna denies…") | The staged role was not picked up. Check `ls-control scan-services` output and the manifest; nothing vendor-side was created. |
| `vtCapture_init status 11` | Luna permission denied (as Prysm's own message says): the role covers the precheck names but not the library's. Stop; compare with the installed worker's role. |
| `vtCapture_init status 17` or `1` | Not ready (17, as in hyperion-webos), or the session was finalized (1). Rerun once with video on screen; Prysm itself waits about a second and retries. |
| Extent 40, guards intact, no pointer writes | Layout A, or C in 40 bytes. Check the words below. |
| Extent 44, last word plausible as a rate (24–120) or a dump location (0–5) | A′. Name the extra field by comparing it with the `--preprocess` run, and with a second run on another input if that is possible. |
| Scratch blocks changed | Layout B. Each changed block's bytes are a value behind that pointer. |
| Nothing written, status 0 | The struct is input, or it is filled only later. Run `--preprocess`. |
| Non-zero status | Note the code. Run `--preprocess`; if it still fails, the call needs something capture-related. Stop and report rather than add `process`. |
| Guard after changed | Larger than 256 bytes. Raise `BODY_WORDS` and rerun. |

Under layout A, these values in the dump would confirm each field:

| Offset | Field | Confirms if | Contradicts if |
| --- | --- | --- | --- |
| 0 | flags | Only bits 0x01–0x20 are set. `SCALE_DOWN` (0x08) is set, since every measured capture is a downscale of 3840×2160. | Bits above 0x20, or a value that looks like a size or pointer. |
| 4, 8 | scale-up limit | Even sizes at or above 1920×1080, likely 1920×1080 or 3840×2160. | Below 1280×720 (measured working). |
| 12, 16 | scale-down limit | A small even size. A value at or below 320×180 can be read as a minimum output size. Amlogic's default is 360×240. | Above 320×180 can only be a minimum if the measured 320×180 captures were wrong. It would then mean something else, such as an input size or a ratio. |
| 20 | frame buffers | 1–8 (Prysm asks for 3; Amlogic reports 5). | 0 or above 16. |
| 24–31 | max_res | x = 0, y = 0, and w×h equal to the scale-up limit or 1920×1080. | x or y not 0. |
| 32 | plane count | 2 (Prysm receives two-plane NV12). | Other than 1–3. |
| 36 | pixel format | 1 (YUV420 semi-planar = NV12). | Other than 0–10. |

The NV12 fields (offsets 32 and 36) are the strongest check, because Prysm
already knows the answer: frames arrive as two-plane NV12. If they read 2 and
1, layout A holds. Read the sizes after that.

### Follow-up sweep (design only, a separate session)

Purpose: confirm that the reported limits are the ones the driver enforces.
This needs `vtCapture_process` and short captures, so it is a separate session
with its own go-ahead. Run it as a probe mode that reuses `VtCapture` through
the public API (`VtCapture::open`, `acquire`, `release`), one open per case.

- **Sizes** (fps 30, dump 2):
  - 320×180, 640×360, 1280×720 (known good);
  - 1920×1080;
  - the reported maximum, and the reported maximum + 2 in each axis (expected
    refusal);
  - the reported scale-down limit, and the limit − 2 (expected refusal or
    upscale);
  - 2×2 (today's `MODE` minimum).
- **Rates** (at 640×360): 1, 10, 24, 25, 30, 50, 60 and 61. Record the frame
  intervals over 60 frames. The struct has no rate, so this shows what
  `setCaptureFrameRate` does with each value.
- **Buffers** (at 640×360, 30 fps): 1, 3, the reported count, and the count + 1.
- **Each case records:**
  - the preprocess status;
  - the `planeInfo` stride, region and active region;
  - whether the frame size equals the size asked;
  - the frame interval mean, p95 and max over 60 frames;
  - the release status.
- **Safety limits:**
  - Prysm stopped; the capture lock held for the whole sweep;
  - one tap at a time, fully released before the next;
  - Prysm's 5 s per-call watchdog and 10 s preparation limit;
  - at most 60 frames or 5 s per case, and at most 30 cases;
  - never larger than 3840×2160 or above 120 fps;
  - even sizes only;
  - stop the whole sweep at the first hang, non-zero release or unexpected
    `dmesg` capture error;
  - finish by checking that Prysm restarts and reports `session: running`.

## Intended code change afterwards (design only)

1. **`capture-webos-sys`:**
   - Add a `#[repr(C)] CapabilityInfo` matching the decoded layout, and
     extend the `abi.cpp` layout test to cover it, with the header corrected
     to the decoded fields.
   - Load `vtCapture_capabilityInfo` in `VtApi`, and add a
     `prysm_vt_capability` bridge (or reuse the struct bridge's shape).
   - Expose `VtCapture::capabilities() -> Result<Capabilities, Error>`,
     called once in `prepare` right after a successful init.
   - `Capabilities` is a validated value: maximum size, minimum size,
     buffers, flags, plane count and format, built only by a `decode`
     function with typed refusals.
   - Nonsense is refused:
     - sizes that are odd, zero or above 7680×4320;
     - a minimum above the maximum;
     - unknown flag bits;
     - buffers 0 or above 16;
     - a plane count or format that is not NV12.

     A refusal is `Error::Invalid` with its cause, never a panic.
2. **`capture-webos`:**
   - The native thread publishes the first valid `Capabilities` into a shared
     cell that `WebosCapture` holds next to `VideoSignal`. It is per process,
     since the TV does not change.
   - `modes` returns `MODE` intersected (`Mode::and`) with the reported spans,
     with fps left at 1–60.
   - It falls back to `MODE` when nothing has been read yet, when the call
     failed or returned an unknown status, or when `decode` refused the
     values. That case is logged once at warn, with the raw words at debug.
   - `validate` stays against the static `MODE`. Settings then load the same
     way before the first open and on any TV, the schema bounds remain true,
     and nothing about the configuration depends on whether the driver was
     reachable.
   - A size outside the reported limits but inside `MODE` is still sent to
     the driver as today. Its refusal is reported as now. A later change can
     decide whether `open` should clamp it instead.
   - The buffer count becomes `min(3, reported)`.
3. **Tests without a TV:**
   - `decode` unit tests on the probe's recorded dump, committed under
     `evidence/` as hex, plus edge cases (zero, odd and huge values, unknown
     flags, the guard pattern);
   - the `abi.cpp` size and offset test;
   - a fake `capabilityInfo` in the existing `FAKE_API` tests, writing the
     fixture, and one returning non-zero;
   - a `capture-webos` test with a fake backend that reports capabilities,
     checking that `modes` narrows to them and falls back to `MODE` for
     none or for nonsense;
   - the existing schema test, which keeps `validate` unchanged.

## Limits

- No TV was available. Every statement about this TV's values is a
  prediction. Facts about the library's symbols come from the earlier symbol
  dump of one firmware, webOS TV 10.3.2.
- The kernel struct comes from Amlogic trees. The measured TV is Realtek, so
  the field order should match and the values will not.
- The probe was built and its report logic tested on macOS only, against a
  fake library, through the real C++ boundary. **It was not compiled for
  armv7:** the crate's `build.rs` needs the webOS SDK's C++ compiler
  (`arm-linux-gnueabi-g++` was not found), so neither `cargo check` nor
  `cargo build --target armv7-unknown-linux-gnueabi` could run.
- `run-tv.sh` has not run. It passes `bash -n` only; shellcheck was not
  available.

## Licence

The field names and layout facts come from the linked headers: MIT for
tv-native-apis and hyperion-webos, GPL-2.0+ OR MIT for the Amlogic copy of
LG's v4l2-ext header. No header was copied. The probe declares only the call
signatures already in `native/vtCaptureApi_c.h` (`LICENSE.tv-native-apis`).
