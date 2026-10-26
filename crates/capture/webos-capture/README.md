# Prysm webOS capture

Minimal native sender for a **rooted LG TV**. Captures video and, when available,
the TV UI, then sends RGB24 frames to Prysm. No Hyperion/HyperHDR server, Luna
service, TV settings UI, HDR switching, or video encoder is involved.

The initial target is webOS 25 (10.x), **not yet verified on hardware**. Capture
uses `unicapture` and the model-specific backends from MIT-licensed
`webosbrew/hyperion-webos`, pinned to the same revision as `resources/piccap`:
`00c932f092084b234b3f619b060a98fa134c12bd`. Native API definitions and libyuv
remain upstream dependencies. Root access does not guarantee capture support.

## Build

Install the [buildroot-nc4 webOS SDK](https://github.com/openlgtv/buildroot-nc4)
and run its `relocate-sdk.sh`. Configure with its ARM toolchain:

```sh
cmake -S crates/capture/webos-capture -B crates/capture/webos-capture/build \
  -DCMAKE_TOOLCHAIN_FILE=/path/to/sdk/share/buildroot/toolchainfile.cmake \
  -DCMAKE_BUILD_TYPE=Release
cmake --build crates/capture/webos-capture/build --parallel
cmake --install crates/capture/webos-capture/build --prefix "$PWD/crates/capture/webos-capture/dist"
```

The first configure/build fetches pinned upstream sources. `dist` contains the
sender, four backend shared libraries, and license notices. This is a native
executable for initial SSH testing, **not an installable IPK yet**.

## Prysm configuration

Replace the `[input]` table in your existing Prysm TOML:

```toml
[input]
type = "webos"
listen = "0.0.0.0:19400"
```

Keep your existing output and processing settings. Enable Screen sync in the
Prysm panel to open the listener. Stopping Screen sync closes it; the TV sender
will retry until it is enabled again. Incoming dimensions are authoritative;
`capture_resolution` does not resize or configure the TV. Set it to `[320, 180]`
for a matching initial preview placeholder, if desired.

This input uses a Prysm-specific protocol; stock PicCap cannot send to it.
There is no authentication or encryption. Use a trusted LAN and restrict port
19400 to your TV with the host firewall.

## First TV test

Stop PicCap or any other TV capture process before starting this executable.
Copy the distribution into a directory writable by root:

```sh
ssh root@TV_IP 'mkdir -p /media/developer/prysm-capture'
scp -r crates/capture/webos-capture/dist/. root@TV_IP:/media/developer/prysm-capture/
ssh -t root@TV_IP
cd /media/developer/prysm-capture
chmod +x prysm-webos-capture
LD_LIBRARY_PATH="$PWD${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
  ./prysm-webos-capture -a PRYSM_MACHINE_IPV4
```

Leave it in the foreground to read errors. Ctrl+C requests shutdown and releases
capture. Test an HDMI source and ordinary TV menus first, using Prysm's egui
output if you want to inspect received pixels. Protected internal-app video may
be unavailable. Captured HDR pixels are not automatically tone-mapped.

Defaults are 320×180 at 30 fps with video/UI backend auto-detection. Options:

| Option | Meaning |
| --- | --- |
| `-a IPV4` | Required Prysm receiver IPv4 address |
| `-p PORT` | TCP port, default 19400 |
| `-w WIDTH -h HEIGHT` | Even dimensions, 2–1920 × 2–1080 |
| `-f FPS` | 1–60, default 30 |
| `-b FILE.so` | Explicit video backend instead of auto-detection |
| `-u FILE.so` | Explicit UI backend; `none` disables UI capture |
| `-q MASK` | Upstream model quirk bitmask, decimal or `0x…` |

Use quirks verified for your TV; they change native capture behavior. Start at
the default resolution before trying 192×108 to reduce network traffic.

Autostart, standby/wake integration, and IPK packaging are deliberately deferred
until capture has been validated on the target TV. The upstream capture workers
can block inside native APIs; shutdown timing needs hardware verification.

## Wire protocol v1

Each TCP frame is a 16-byte header followed by tightly packed RGB24 pixels:

| Offset | Bytes | Value |
| --- | --- | --- |
| 0 | 4 | ASCII `PRY1` |
| 4 | 4 | Width, unsigned big-endian |
| 8 | 4 | Height, unsigned big-endian |
| 12 | 4 | Payload byte length, unsigned big-endian (`width × height × 3`) |
| 16 | length | RGB24, rows top-to-bottom |

There are no registration messages or application replies. Maximum dimensions
are 1920×1080. The receiver validates the header before allocating pixels,
disconnects malformed/stalled clients, accepts reconnections, and publishes only
the latest complete frame. Only one TV connection is handled at a time.

The sender reuses capture buffers directly during sending, handles partial writes,
and has no application frame queue. Connect and frame writes each have a 100 ms
deadline; failure closes the connection and retries after one second. TCP socket
buffers can still retain frames briefly; this is not a hard end-to-end latency
guarantee. No compression, resampling, smoothing, or LED layout runs on the TV.

## Host checks

```sh
python3 crates/capture/webos-capture/tests/test_transport.py
cargo test -p prysm-capture -p prysm-contract --locked
```

These cover the sender's wire bytes, retry and write deadline, receiver parsing,
fragmented frames, invalid payloads, reconnection, shutdown, and configuration.
They do not establish capture compatibility or color accuracy on an LG TV.
