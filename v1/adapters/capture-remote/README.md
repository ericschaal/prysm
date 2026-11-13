# prysm-capture-remote

Frames from a capture-only host (webOS slim) to a lighting host (headless) over
the LAN. The lighting host decides what it wants: its settings hold a
`StreamRequest` (size, rate, menus, encoding) that the sender follows. Both
ends are blocking adapters on `std::net`:

- `RemoteReceiver` registers source kind `"remote"`:

  ```json
  { "capture": { "kind": "remote", "listen": "0.0.0.0:7380",
                 "width": 320, "height": 180, "fps": 30,
                 "include_ui": true, "encoding": "h264_hardware" } }
  ```

  Only `listen` is required; it must have a fixed, non-zero port. The other
  fields are the request, with the defaults shown: an even `width` (2 to 1920)
  and `height` (2 to 1080), at least 16x16 for H.264; `fps` 1 to 60;
  `include_ui` blends the sender's on-screen menus into the picture;
  `encoding` is `h264_hardware` (the TV's encoder), `h264_software`
  (OpenH264 on the TV) or `raw`. `open` binds and starts one receive thread,
  which also decodes H.264. `next_frame(timeout)` returns the newest complete
  frame (older untaken frames are replaced, never queued) or `Pull::Idle` on
  timeout, and fails once with the sender's reason when the sender refuses
  the request (see below). `close` pauses the sender, stops the thread and
  releases the socket within about `POLL_INTERVAL` (50 ms).
  `RemoteSource::set_wanted(false)` pauses the sender without closing.
- `RemoteSender::connect(peer)` is a `FrameSink`. It learns the request in
  the handshake (`request()`), and `send` encodes each frame with exactly the
  requested encoder from [`prysm-h264`](../h264/src/lib.rs), opened at the
  first frame. Any error is final for that connection. `deliver` (what `send`
  calls) returns a `SendError`: `Encoder` when the requested encoder cannot
  start or failed, after the sender refused the encoding to the receiver
  (`refuse_encoding`), else `Connection`. `refuse(reason)` refuses the request for any other
  reason. Nothing switches
  encoder or encoding on its own.

A new request needs a new connection: the engine reopens the source when the
capture settings change, which closes the connection, and the sender's next
connection (the slim TV worker retries every second) gets the new request.

The slim TV worker wraps the sender in an uplink that reconnects at most
every second and reports the wait, because the receiver listens only while
its host is lighting. It captures at the requested size, rate and menus,
reopening the TV capture when they change, and closes the capture while the
receiver has paused the stream (it keeps reading control messages through
`FrameSink::tick`, so a resume reaches it); a receiver that pauses and then
closes, as on lighting stop, leaves the capture closed until a receiver
connects again. The receiver may be given by host name: the worker resolves
it at every connection attempt. When the requested encoder fails
it reports `failure_kind: encoder` and refuses every later connection with the
same request (the receiver's source fails with the reason, so lighting stops
with it in the portal) until the receiver asks for something else.

To watch a TV's stream on a desktop without lighting anything (one line per
second; `--pause-at N` pauses the sender at second N and resumes it three
seconds later; `--width`, `--height`, `--fps` and `--encoding` set the
request):

```sh
cargo run --manifest-path v1/Cargo.toml -p prysm-capture-remote --example receive -- 0.0.0.0:7380 60
```

## Wire protocol (version 3)

One TCP connection carries one stream: an 8-byte hello from the sender, the
receiver's 16-byte reply with the request, then frames (or one refusal) from
the sender, each a 16-byte header followed by its payload, and one-byte
control messages from the receiver. TCP gives ordering and backpressure for
free and a frame is far larger than a UDP datagram, so there is no need for
framing, fragmentation or loss handling of our own. All integers are
big-endian.

**Hello** (sender → receiver, 8 bytes) and **reply** (receiver → sender, 16
bytes):

| Offset | Size | Hello | Reply |
| --- | --- | --- | --- |
| 0 | 4 | magic `PRSM` | magic `PRSM` |
| 4 | 1 | version `3` | the receiver's version |
| 5 | 1 | zero | status: `0` accepted, `1` unsupported version |
| 6 | 2 | zero | zero |
| 8 | 1 | — | encoding: `0` raw, `1` H.264 hardware, `2` H.264 software |
| 9 | 1 | — | `include_ui`: `0` or `1` |
| 10 | 2 | — | width (`u16`) |
| 12 | 2 | — | height (`u16`) |
| 14 | 1 | — | fps |
| 15 | 1 | — | zero |

The request bytes are zero unless the status accepts the stream. The request
is fixed for the whole connection. A receiver that refuses the hello replies
and closes the connection, and logs the refusal once (`warn`) until a stream
is accepted. The sender reads the first 8 bytes of the reply before the
rest, so a receiver of another version is recognised by its version byte
alone. The sender's `connect` fails with a typed `WireError` cause:
`Version { ours, theirs }` for a receiver of another version, `Request` for a
request it cannot understand (unknown codes, values outside the limits above),
and `NoReply` for a peer that closes or stays silent for `CONNECT_TIMEOUT`. A
version-2 sender's hello (byte 5 is its encoding) is refused the same way, and
a version-2 receiver answers a version-3 hello with an 8-byte reply naming
version 2. The slim TV worker shows the cause as its waiting reason.

**Refusal** (sender → receiver, instead of a frame, then the sender closes):
a 16-byte header with format `0`, flags (offset 3: `1` when the requested
encoding is the cause, its encoder cannot start or failed; else `0`), every
other byte zero except the length (offset 12, `u32`, 1 to 1024), followed by
that many bytes of UTF-8: why the sender cannot produce the requested stream,
such as `cannot encode frame: hardware H.264 encoder cannot start: …`. The
receiver's next `next_frame` fails once. An encoding refusal is
`Error::Invalid` for field `capture.encoding` with the reason `the sender
cannot stream 320x180 at 30 fps with menus as h264_hardware: <reason>`, so
the portal can show it on that setting; any other refusal is an adapter
failure with that context, caused by `WireError::Refused(reason)`.
`RemoteSender::refuse_encoding` and `refuse` send the two.

**Frame header:**

| Offset | Size | Field | Value |
| --- | --- | --- | --- |
| 0 | 1 | format | `1` RGB24, `2` BGR24, `3` YUYV, `4` NV12 (`0`: a refusal) |
| 1 | 1 | YUV range | `0` (RGB/BGR), `1` limited, `2` full |
| 2 | 1 | YUV matrix | `0` (RGB/BGR), `1` BT.601, `2` BT.709, `3` BT.2020, `4` SMPTE 240, `5` BT.470-6, `6` FCC |
| 3 | 1 | flags | raw: `0`. H.264: bit 0 *keyframe*; other bits `0` |
| 4 | 4 | width | `u32`, 1..=4096 |
| 8 | 4 | height | `u32`, 1..=4096 |
| 12 | 4 | payload length | `u32`, at most 6,220,800 (1080p RGB). Raw: exactly `width * height * 3` (RGB/BGR), `* 2` (YUYV) or `* 3 / 2` (NV12). H.264: 1 or more |

The format describes the frame's pixels; NV12 is the Y plane (`width` bytes
per row) followed by the interleaved UV plane (`width` bytes per row, `height /
2` rows), with even width and height. MJPEG and custom YUV matrices are
rejected by the sender. A frame's size is its own: the receiver does not require
the requested one (the slim TV captures at it).

**H.264 stream** (encoding `1` or `2`). Each payload is one access unit (Annex B, Baseline,
progressive, one picture) of a frame whose decoded pixels the header
describes: limited-range BT.709 NV12 only, the webOS capture's native format.
An encoder *session* starts with SPS, PPS and an IDR picture of the current
frame, flagged *keyframe*; the receiver decodes it with a new decoder, so no
reference crosses sessions. A session starts on each connection, after a
resume and on a keyframe request; with the software encoder there are no
other IDR pictures, since TCP loses nothing. Rate control may skip a frame
(nothing is sent). Before OpenH264 sees an access unit the receiver checks
its NAL types and every SPS (size equal to the header's, Baseline, progressive,
at most 1920x1088, limited-range BT.709 colour signal), and decodes into a
reused `FramePool` frame, so the processor sees NV12 exactly as from a raw
stream. A picture that does not decode (a lost reference, a corrupt picture,
or a stream that does not start with a keyframe) is dropped, the receiver
sends *keyframe* once and skips pictures until the next keyframe; a keyframe
that does not decode drops the connection.

**Control messages** (receiver → sender, one byte each, any time after the
reply; anything else fails the sender's connection):

| Byte | Message | Sender |
| --- | --- | --- |
| `1` | pause: frames are not wanted | drops frames unencoded and closes its encoder session (releasing a hardware encoder) |
| `2` | resume | sends again; the next H.264 picture starts a new session |
| `3` | keyframe | the next H.264 picture starts a new session |

The receiver sends pause when its source is closed (lighting stops) and when
`set_wanted(false)` is called, and resume after `set_wanted(true)`. The
sender reads pending messages without blocking at the start of each `send`
and in `tick` (a sender with nothing to send, such as the paused slim TV); a
receiver that has closed shows as `WireError::Closed`.

- **Validation before allocation.** The receiver checks magic and version once
  per stream, then format, flags, dimensions and the payload length from each
  frame header alone; raw payloads go straight into a reused `FramePool`
  buffer, H.264 payloads into one reused buffer of at most the maximum payload.
- **Malformed input** (bad header, wrong length, EOF or I/O error mid-frame, a
  keyframe that does not decode, a malformed refusal) drops that connection
  without panicking; the receiver keeps listening.
- **One sender at a time.** A new connection replaces the current one, so a TV
  that restarts without closing its old socket is accepted immediately. A
  sender disconnecting is not an error.
- **Timeouts.** Sender: 3 s connect, 3 s for the reply, 2 s per write (a receiver that stops
  reading fails the send). Receiver: reads wake every 50 ms to check for
  `close`, newer connections and pause/resume; a quiet connection is kept open.
- **Bandwidth.** Raw: the TV sends its capture tap's native limited BT.709
  NV12 without converting it; 320x180 NV12 at 30 fps is 86,416 bytes per frame
  (header included), about 20.7 Mbit/s; 640x360 about 83 Mbit/s. H.264 targets
  2 Mbit/s (the qualified 640x280 at 30 fps setting). In the loopback tests at
  640x280/30 the software encoder used 0.12 Mbit/s for a moving ramp and
  3.7 Mbit/s for full-frame noise in the first second, which starts with an
  IDR (raw: 64.5 Mbit/s).

**Security:** there is no authentication or encryption. Anyone on the network
can send frames to the receiver or take over its connection. Use it only on a
trusted LAN.
