# Prysm webOS capture

Rust CLI for capturing video, TV UI, or both on a **rooted LG TV**, and streaming
frames directly to Prysm. Capture scheduling, pixel processing, recovery, CLI,
and transport are Rust. The four native backends load LG's system libraries
directly. There is no Hyperion daemon, C sender, CMake build, or backend plugin
ABI. The receiver implements one bounded raw/LZ4 protocol.

The native adapters derive from MIT-licensed [hyperion-webos](https://github.com/webosbrew/hyperion-webos)
revision `00c932f092084b234b3f619b060a98fa134c12bd`; retain the supplied license
notices. C/C++ is limited to vendor API headers, a typed exception trampoline
for LG's C video API, and an ABI/exception test fixture. LG can throw C++
exceptions through its C API, so a C++ boundary is necessary to contain them.
No type-erased calls, numeric operation dispatcher, or function-pointer casts
are used. All unsafe Rust is confined to `src/native`; the rest of the sender
denies unsafe code at compile time.

The Luna setup and native video startup were previously verified on an LG
43QNED87A6D.BEUFLJP running webOS 25 (10.3.2-34), with UI disabled. The Rust sender
has also completed a short 640×240/30 fps test with both layers and LZ4 on that TV.
Root access does not guarantee compatibility with other drivers or correct colors.

## Build for webOS

Install and relocate the [buildroot-nc4 webOS SDK](https://github.com/openlgtv/buildroot-nc4),
then from the repository root:

```sh
rustup target add armv7-unknown-linux-gnueabi
WEBOS_SDK=/path/to/arm-webos-linux-gnueabi_sdk-buildroot \
  ./crates/capture/webos-capture/build-webos.sh
```

The script builds the standalone sender with the SDK C++ compiler and linker,
an explicit sysroot, Cortex-A9 code generation, and the **soft-float ABI**
(`gnueabi`, not `gnueabihf`). The `webos` Cargo profile enables optimization, LTO,
one codegen unit, and stripped symbols. The executable uses `/lib/ld-linux.so.3`.
Rust normally targets glibc 2.17+, while this SDK supplies glibc 2.12; the linker
explicitly includes the SDK's `libglibc_polyfills.a` for `getauxval`. This is a
webOS compatibility build; runtime behavior still needs hardware validation.

`dist` contains the executable, permissions, and license notices. LG's capture
libraries, `libstdc++.so.6`, and `libgcc_s.so.1` come from the TV. No Hyperion
libraries or upstream checkout is required. IPK packaging, autostart, and
standby/wake integration are not implemented.

## Usage

Start Prysm with this input and enable Screen sync to open the receiver:

```toml
[input]
type = "webos"
listen = "0.0.0.0:19400"
```

The sender takes one receiver IP address and port (IPv4 or bracketed IPv6):

```sh
# Lowest TV CPU cost: stream the video planes without color conversion.
prysm-webos-capture 192.168.1.10:19400 --source video

# Composite the TV's UI onto video; this is the default capture mode.
prysm-webos-capture 192.168.1.10:19400 --source both --compression lz4

# UI capture works independently, without opening a video driver.
prysm-webos-capture 192.168.1.10:19400 --source ui --ui-backend halgal

prysm-webos-capture --help
```

Defaults are 320×180 at up to 30 fps, both layers, automatic backend selection,
and no compression. `--width`, `--height`, and `--fps` are checked before any
native library is opened. Dimensions must be even and within 2–1920 × 2–1080;
fps must be 1–60. The receiver's `capture_resolution` does not configure the TV.

| Option | Choices / purpose |
| --- | --- |
| `--source` | `video`, `ui`, `both` |
| `--compression` | `none`, `lz4` (lossless, raw fallback) |
| `--video-backend` | `auto`, `vtcapture`, `dile` |
| `--ui-backend` | `auto`, `gm`, `halgal` |
| `--capture-point` | `display`, `scaler-input`, `scaler-output`, `blended`, `osd`, `histogram` |
| `--force-capture` | Use an installed `/dev/forcecapture` kernel module when vtcapture is not ready |
| `--dile-create-ex` | Use the alternate DILE constructor on TVs that need it |
| `--no-freeze` | Skip DILE buffer freezing; may produce tearing |

Hardware settings are named and typed; there is no numeric quirk mask. Use only
settings verified for your TV. DILE supports `display` and `scaler-output` and
rejects dimensions below its reported minimum. Backend flags are relevant only
when their layer is selected. With both layers selected, unavailable UI falls
back to video. Explicit UI-only capture reports UI initialization failures.

## First TV run

Stop PicCap or any other capture process. Copy the distribution:

```sh
ssh root@TV_IP 'mkdir -p /media/developer/prysm-capture'
scp -r crates/capture/webos-capture/dist/. root@TV_IP:/media/developer/prysm-capture/
ssh -t root@TV_IP
```

Apply the temporary Luna setup below if needed. On the TV:

```sh
cd /media/developer/prysm-capture
./prysm-webos-capture PRYSM_MACHINE_IPV4:19400 \
  --source video --video-backend vtcapture --width 320 --height 180
```

Ctrl+C and SIGTERM request shutdown and resource cleanup. Native driver calls
can block, so shutdown latency must be measured on hardware. Test HDMI and
ordinary menus with Prysm's desktop preview first. Protected video may be
unavailable. Captured HDR pixels are not automatically tone-mapped.

## Temporary Luna capture permissions

`vtcapture exception: LSRegisterPubPriv FAILED` can mean Luna denies
registration of the executable's capture-client identity, even when run as root.
The supplied `permissions/role.json` grants capture-client and media resource
manager access to exactly `/media/developer/prysm-capture/prysm-webos-capture`.
Moving the executable requires updating `exeName` in that role.

This is the setup verified on the model/firmware above. It overlays a manifest
using a bind mount; the original system file is not overwritten. Reloading
permissions affects the shared Luna configuration. Review the files first;
this is not an automatic installer or a persistent boot setup.

On the TV, prepare temporary copies (stop if this directory already exists):

```sh
mkdir -m 700 /tmp/prysm-luna-test &&
cp /media/developer/prysm-capture/permissions/role.json \
   /media/developer/prysm-capture/permissions/screen-capture-webos.manifest.json \
   /tmp/prysm-luna-test/
```

Compare the proposed manifest with your TV's original:

```sh
diff -u /usr/share/luna-service2/manifests.d/screen-capture-webos.manifest.json \
        /tmp/prysm-luna-test/screen-capture-webos.manifest.json
```

The only substantive change must be adding `/tmp/prysm-luna-test/role.json`
to `roleFiles`. If any existing entries differ, stop and adapt the template to
preserve your TV's entries. Also stop if the manifest is already bind-mounted;
check with `mount | grep screen-capture-webos.manifest.json`.

After reviewing the diff, apply it:

```sh
mount --bind \
  /tmp/prysm-luna-test/screen-capture-webos.manifest.json \
  /usr/share/luna-service2/manifests.d/screen-capture-webos.manifest.json &&
ls-control scan-services
```

This does not start capture. Run the foreground video-only test above afterward.
The mount and `/tmp` files do not survive a normal reboot; no autostart hook is
installed. Do not delete the role file while the overlay is active.

To roll back, stop Prysm capture with Ctrl+C, then restore the original manifest
view, reload permissions, and remove only the files added here:

```sh
umount /usr/share/luna-service2/manifests.d/screen-capture-webos.manifest.json &&
ls-control scan-services &&
rm /tmp/prysm-luna-test/role.json \
   /tmp/prysm-luna-test/screen-capture-webos.manifest.json &&
rmdir /tmp/prysm-luna-test
```

The rollback sequence has not yet been verified on the target TV. Do not replace
`scan-services` with `scan-volatile-dirs`: the latter only adds manifests and
does not reliably remove previously loaded permissions.

Once the log reaches `Capturing …`, `Connection refused` refers to
the receiver: start Prysm with the webOS input and enable Screen sync (or
**Start capture** in the desktop preview) to open TCP port 19400.

## Performance and protocol

Video-only capture packs NV12/NV16 directly from the driver. Color conversion
runs on the Prysm host. NV12 uses half the bandwidth of RGB24: at 640×360/30 fps,
raw pixels need 83 Mbit/s instead of 166 Mbit/s, before network overhead.
320×180 NV12 needs about 21 Mbit/s. UI blending converts video directly to RGB
and blends premultiplied BGRA in place, without an ARGB intermediate frame.
The Rust `yuv` dependency uses a scalar ARMv7 conversion path; measure UI-enabled
CPU usage on the TV before increasing resolution.

Buffers are reused. Checked slice copies omit native stride padding, avoid
zero-filling, and release frozen native frames before network writes. Failed
acquisition does not send stale pixels. Missed ticks are skipped. Video open and
capture errors retry after one second, including at startup; UI frames continue
while video is unavailable. Driver errors remain logged until capture succeeds
or the sender is stopped. Vtcapture status 2 means initialization is still pending;
the sender polls the same driver for up to two seconds before restarting. Its
release function deletes the driver without returning a status; only exceptions
are treated as release failures. Every successful acquisition is copied, even when
the driver reuses a buffer address. Without a frame sequence or timestamp, a repeated
address cannot distinguish a fresh frame from a stalled driver, so it does not
trigger a restart.

`--compression lz4` independently compresses each frame using a pure Rust block
codec and reusable storage. Expanded frames are sent raw. Compression is off by
default because noise/film grain can cost CPU without saving bandwidth. Run the
benchmark below on representative hardware; synthetic flat/gradient/noise data
does not establish TV performance. On the development ARM64 Mac, 640×360 NV12
synthetic encoding took roughly 9–11 µs per frame; these are host measurements.

There is no frame queue. Connect and complete-frame writes each have a 100 ms
deadline; partial writes close the connection and retry after one second. During
network backoff, video polling continues but UI capture and composition are skipped.
Safe `nix` polling preserves one deadline across every partial write. TCP buffers can
still retain frames briefly, so this is not a hard end-to-end latency guarantee.

The sole wire format is a 24-byte header followed by its payload:

| Offset | Bytes | Value |
| --- | --- | --- |
| 0 | 4 | ASCII `PRY2` |
| 4 | 4 | Width, unsigned big-endian |
| 8 | 4 | Height, unsigned big-endian |
| 12 | 4 | Format: `0` RGB24, `1` NV12, `2` NV16 |
| 16 | 4 | Codec: `0` raw, `1` LZ4 block without a size prefix |
| 20 | 4 | Payload byte length, unsigned big-endian |
| 24 | length | Tightly packed pixels or their LZ4 block |

NV12 is Y followed by interleaved UV at half height; NV16 uses full-height UV.
Native padding is omitted. Maximum dimensions are 1920×1080. NV12 requires
even width/height and NV16 requires even width. Allocation bounds and decoded
length come from validated dimensions, never a compressed size prefix. Unknown
formats/codecs, invalid lengths, corrupt LZ4, truncated frames, and stalled peers
disconnect. Receiver scratch buffers are reused; only complete latest frames are
published. Both sender and receiver must use this protocol; no legacy protocol
or command-line aliases remain.

There is no authentication or encryption. Bind on a trusted LAN interface and
restrict port 19400 to the TV. One TV connection is handled at a time.

## Verification

```sh
cargo test --locked -p prysm-webos-capture -p prysm-webos-protocol -p prysm-capture
cargo run --release -p prysm-webos-protocol --example compression
```

Tests cover typed CLI validation/help, native ABI layouts, C++ exception
containment, reused video buffers, pending initialization without premature restart,
startup retries with UI output, DILE unfreezing after acquisition failures,
HAL surface cleanup after mapping failure, skipped UI
work during network backoff, padded planes, color conversion/blending,
raw/LZ4 boundaries, malformed input, retry/backoff, complete-frame deadlines,
and receiver cancellation. CI also runs sender/protocol tests as ARM32 soft-float
executables under QEMU. Cross-build separately with the actual SDK script.
These checks do not load LG drivers or establish hardware performance or colors.
