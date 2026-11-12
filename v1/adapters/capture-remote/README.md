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

## Wire protocol (version 1)

One TCP connection carries a stream of frames; each frame is a 20-byte header
followed by the raw pixels. TCP gives ordering and backpressure for free and a
frame is far larger than a UDP datagram, so there is no need for framing,
fragmentation or loss handling of our own.

| Offset | Size | Field | Value |
| --- | --- | --- | --- |
| 0 | 4 | magic | `PRSM` |
| 4 | 1 | version | `1` |
| 5 | 1 | format | `1` RGB24, `2` BGR24, `3` YUYV |
| 6 | 1 | YUV range | `0` (RGB/BGR), `1` limited, `2` full |
| 7 | 1 | YUV matrix | `0` (RGB/BGR), `1` BT.601, `2` BT.709, `3` BT.2020, `4` SMPTE 240, `5` BT.470-6, `6` FCC |
| 8 | 4 | width | big-endian `u32`, 1..=4096 |
| 12 | 4 | height | big-endian `u32`, 1..=4096 |
| 16 | 4 | payload length | big-endian `u32`; must equal `width * height * bytes per pixel` and be at most 6,220,800 (1080p RGB) |

MJPEG and custom YUV matrices are rejected by the sender.

- **Validation before allocation.** The receiver checks magic, version, format,
  dimensions and the exact payload length from the header alone, then reads the
  payload into a reused `FramePool` buffer.
- **Malformed input** (bad header, wrong length, EOF or I/O error mid-frame)
  drops that connection without panicking; the receiver keeps listening.
- **One sender at a time.** A new connection replaces the current one, so a TV
  that restarts without closing its old socket is accepted immediately. A
  sender disconnecting is not an error.
- **Timeouts.** Sender: 3 s connect, 2 s per write (a receiver that stops
  reading fails the send). Receiver: reads wake every 50 ms to check for
  `close` and newer connections; a quiet connection is kept open.
- **Uncompressed.** The root `prysm-webos-protocol` LZ4 path would need a new
  dependency and a scratch buffer per frame; at ambient-lighting resolutions raw
  is cheap enough. 320x180 RGB24 at 30 fps is 172,820 bytes per frame,
  about 5.2 MB/s (41.5 Mbit/s); YUYV is two-thirds of that. Fine on wired
  Ethernet; on a weak Wi-Fi link prefer a smaller capture or add a codec under a
  new version.

**Security:** there is no authentication or encryption. Anyone on the network
can send frames to the receiver or take over its connection. Use it only on a
trusted LAN.
