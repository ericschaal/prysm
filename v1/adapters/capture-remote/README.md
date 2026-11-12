# prysm-capture-remote

Frames from a capture-only host (webOS slim) to a lighting host (headless) over
the LAN. Both ends are blocking adapters on `std::net`:

- `RemoteSender::connect(peer)` is a `FrameSink`; `send` writes one frame. Any
  error is final for that connection. The slim TV worker wraps it in an
  uplink that reconnects at most every second and reports the wait, because
  the receiver listens only while its host is lighting.
- `RemoteReceiver` registers source kind `"remote"`:

  ```json
  { "capture": { "kind": "remote", "listen": "0.0.0.0:7380" } }
  ```

  `listen` must have a fixed, non-zero port. `open` binds and starts one receive
  thread. `next_frame(timeout)` returns the newest complete frame (older
  untaken frames are replaced, never queued) or `Pull::Idle` on timeout. `close`
  stops the thread and releases the socket within about `POLL_INTERVAL`
  (50 ms).

## Wire protocol (version 2)

One TCP connection carries one stream: an 8-byte hello and the receiver's
8-byte reply, then frames, each a 16-byte header followed by its payload. TCP
gives ordering and backpressure for free and a frame is far larger than a UDP
datagram, so there is no need for framing, fragmentation or loss handling of
our own. All integers are big-endian.

**Hello** (sender → receiver) and **reply** (receiver → sender):

| Offset | Size | Hello | Reply |
| --- | --- | --- | --- |
| 0 | 4 | magic `PRSM` | magic `PRSM` |
| 4 | 1 | version `2` | the receiver's version |
| 5 | 1 | encoding: `0` raw (`1` reserved for H.264) | status: `0` accepted, `1` unsupported version, `2` unsupported encoding |
| 6 | 2 | zero | zero |

The encoding is fixed for the whole stream. A receiver that refuses the hello
replies and closes the connection, and logs the refusal once (`warn`) until a
stream is accepted. The sender's `connect` fails with a typed
`WireError` cause: `Version { ours, theirs }` for a receiver of another version,
`Encoding` for a refused encoding, and `NoReply` for a peer that closes or stays
silent for `CONNECT_TIMEOUT` (a version-1 receiver never replies). A version-1
sender's first frame header reads as hello version `1` and is refused the same
way. The slim TV worker shows the cause as its waiting reason.

**Frame header:**

| Offset | Size | Field | Value |
| --- | --- | --- | --- |
| 0 | 1 | format | `1` RGB24, `2` BGR24, `3` YUYV, `4` NV12 |
| 1 | 1 | YUV range | `0` (RGB/BGR), `1` limited, `2` full |
| 2 | 1 | YUV matrix | `0` (RGB/BGR), `1` BT.601, `2` BT.709, `3` BT.2020, `4` SMPTE 240, `5` BT.470-6, `6` FCC |
| 3 | 1 | flags | `0` |
| 4 | 4 | width | `u32`, 1..=4096 |
| 8 | 4 | height | `u32`, 1..=4096 |
| 12 | 4 | payload length | `u32`, at most 6,220,800 (1080p RGB); for a raw stream exactly `width * height * 3` (RGB/BGR), `* 2` (YUYV) or `* 3 / 2` (NV12) |

The format describes the frame's pixels; NV12 is the Y plane (`width` bytes
per row) followed by the interleaved UV plane (`width` bytes per row, `height /
2` rows), with even width and height. MJPEG and custom YUV matrices are
rejected by the sender. An H.264 stream would keep this header (format: the
decoded pixels, payload: one access unit of variable length) and only change
how the payload is read, so adding it is a new encoding code, not a new
version.

- **Validation before allocation.** The receiver checks magic and version once
  per stream, then format, dimensions and the exact payload length from each
  frame header alone, and reads the payload into a reused `FramePool` buffer.
- **Malformed input** (bad header, wrong length, EOF or I/O error mid-frame)
  drops that connection without panicking; the receiver keeps listening.
- **One sender at a time.** A new connection replaces the current one, so a TV
  that restarts without closing its old socket is accepted immediately. A
  sender disconnecting is not an error.
- **Timeouts.** Sender: 3 s connect, 3 s for the reply, 2 s per write (a receiver that stops
  reading fails the send). Receiver: reads wake every 50 ms to check for
  `close` and newer connections; a quiet connection is kept open.
- **Uncompressed.** The TV sends its capture tap's native limited BT.709
  NV12 without converting it. 320x180 NV12 at 30 fps is 86,416 bytes per frame
  (header included), about 2.6 MB/s (20.7 Mbit/s); 640x360 is 345,616 bytes,
  about 83 Mbit/s. RGB24 would be twice that. Fine on wired Ethernet; on a weak
  Wi-Fi link prefer a smaller capture until the H.264 encoding lands
  (`prysm-3jx.12.2`).

**Security:** there is no authentication or encryption. Anyone on the network
can send frames to the receiver or take over its connection. Use it only on a
trusted LAN.
