# prysm-capture-remote

Frames from a capture-only host (webOS slim) to a lighting host (headless) over
the LAN. Both ends are blocking adapters on `std::net`:

- `RemoteSender::connect(peer, stream)` is a `FrameSink`; `send` encodes and
  writes one frame. `stream` is `Stream::Raw` or `Stream::H264(settings)`
  with exactly one encoder backend from [`prysm-h264`](../h264/src/lib.rs):
  the webOS TV's hardware encoder or OpenH264. Any error is final for that
  connection; one whose cause is a `prysm_h264::H264Error` is the encoder's
  (it cannot start or failed), anything else the connection's. The slim TV
  worker wraps the sender in an uplink that reconnects at most every second
  and reports the wait, because the receiver listens only while its host is
  lighting; an encoder failure instead fails the worker. Nothing switches
  encoder or encoding on its own.
- `RemoteReceiver` registers source kind `"remote"`:

  ```json
  { "capture": { "kind": "remote", "listen": "0.0.0.0:7380" } }
  ```

  `listen` must have a fixed, non-zero port. `open` binds and starts one receive
  thread, which also decodes H.264. `next_frame(timeout)` returns the newest
  complete frame (older untaken frames are replaced, never queued) or
  `Pull::Idle` on timeout. `close` pauses the sender, stops the thread and
  releases the socket within about `POLL_INTERVAL` (50 ms).
  `RemoteSource::set_wanted(false)` pauses the sender without closing.

To watch a TV's stream on a desktop without lighting anything (one line per
second; `--pause-at N` pauses the sender at second N and resumes it three
seconds later):

```sh
cargo run --manifest-path v1/Cargo.toml -p prysm-capture-remote --example receive -- 0.0.0.0:7380 60
```

## Wire protocol (version 2)

One TCP connection carries one stream: an 8-byte hello and the receiver's
8-byte reply, then frames from the sender, each a 16-byte header followed by
its payload, and one-byte control messages from the receiver. TCP gives
ordering and backpressure for free and a frame is far larger than a UDP
datagram, so there is no need for framing, fragmentation or loss handling of
our own. All integers are big-endian.

**Hello** (sender → receiver) and **reply** (receiver → sender):

| Offset | Size | Hello | Reply |
| --- | --- | --- | --- |
| 0 | 4 | magic `PRSM` | magic `PRSM` |
| 4 | 1 | version `2` | the receiver's version |
| 5 | 1 | encoding: `0` raw, `1` H.264 | status: `0` accepted, `1` unsupported version, `2` unsupported encoding |
| 6 | 2 | zero | zero |

The encoding is fixed for the whole stream. A receiver that refuses the hello
replies and closes the connection, and logs the refusal once (`warn`) until a
stream is accepted. The sender's `connect` fails with a typed
`WireError` cause: `Version { ours, theirs }` for a receiver of another version,
`Encoding` for a refused encoding, and `NoReply` for a peer that closes or stays
silent for `CONNECT_TIMEOUT` (a version-1 receiver never replies). A version-1
sender's first frame header reads as hello version `1` and is refused the same
way. The slim TV worker shows the cause as its waiting reason; a refused
encoding is not downgraded.

**Frame header:**

| Offset | Size | Field | Value |
| --- | --- | --- | --- |
| 0 | 1 | format | `1` RGB24, `2` BGR24, `3` YUYV, `4` NV12 |
| 1 | 1 | YUV range | `0` (RGB/BGR), `1` limited, `2` full |
| 2 | 1 | YUV matrix | `0` (RGB/BGR), `1` BT.601, `2` BT.709, `3` BT.2020, `4` SMPTE 240, `5` BT.470-6, `6` FCC |
| 3 | 1 | flags | raw: `0`. H.264: bit 0 *keyframe*; other bits `0` |
| 4 | 4 | width | `u32`, 1..=4096 |
| 8 | 4 | height | `u32`, 1..=4096 |
| 12 | 4 | payload length | `u32`, at most 6,220,800 (1080p RGB). Raw: exactly `width * height * 3` (RGB/BGR), `* 2` (YUYV) or `* 3 / 2` (NV12). H.264: 1 or more |

The format describes the frame's pixels; NV12 is the Y plane (`width` bytes
per row) followed by the interleaved UV plane (`width` bytes per row, `height /
2` rows), with even width and height. MJPEG and custom YUV matrices are
rejected by the sender.

**H.264 stream.** Each payload is one access unit (Annex B, Baseline,
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
sender reads pending messages without blocking at the start of each `send`;
a receiver that has closed shows as `WireError::Closed`.

- **Validation before allocation.** The receiver checks magic and version once
  per stream, then format, flags, dimensions and the payload length from each
  frame header alone; raw payloads go straight into a reused `FramePool`
  buffer, H.264 payloads into one reused buffer of at most the maximum payload.
- **Malformed input** (bad header, wrong length, EOF or I/O error mid-frame, a
  keyframe that does not decode) drops that connection without panicking; the
  receiver keeps listening.
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
