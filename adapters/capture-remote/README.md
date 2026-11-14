# prysm-capture-remote

Frames from a capture-only host (webOS slim) to a lighting host (headless)
through the lighting host's web portal. The lighting host decides what it
wants: its settings hold a `StreamRequest` (size, rate, menus, encoding) that
the sender follows. Both ends are blocking adapters on `std::net`; neither
has a listener of its own:

- `RemoteReceiver` registers source kind `"remote"`:

  ```json
  { "capture": { "kind": "remote",
                 "width": 320, "height": 180, "fps": 30,
                 "include_ui": true, "encoding": "h264_hardware" } }
  ```

  Every field is optional; these are the defaults. An even `width` (2 to
  1920) and `height` (2 to 1080), at least 16x16 for H.264; `fps` 1 to 60;
  `include_ui` blends the sender's on-screen menus into the picture;
  `encoding` is `h264_hardware` (the TV's encoder), `h264_software`
  (OpenH264 on the TV) or `raw`. The host gives its `incoming()`
  [`Handoff`](../../crates/ports/src/handoff.rs) to the web portal
  (`WebPortal::capture_stream`), which answers the WebSocket upgrade at
  `/api/v1/capture/stream` and hands the connection over as a plain blocking
  socket. `open` takes those connections on one receive thread, which also
  decodes H.264. `next_frame(timeout)` returns the newest complete frame
  (older untaken frames are replaced, never queued) or `Pull::Idle` on
  timeout, and fails when the sender refuses the request (see below). `close`
  pauses the sender and stops the thread within about 50 ms. Closing is the
  only pause: the next source (lighting on again) gets a new connection,
  which starts unpaused.
- `RemoteSender::connect(url, address)` is a `FrameSink`: it opens the
  WebSocket at `url` (`ws://host:port/api/v1/capture/stream`, or `wss://` for
  an `https://` Prysm, verified against the system's certificate
  authorities) by connecting to `address`, where the caller resolved the
  host. It learns the request first (`request()`), and `send` encodes each
  frame with exactly the requested encoder from
  [`prysm-h264`](../h264/src/lib.rs), opened at the first frame. Any error is
  final for that connection. `deliver` (what `send` calls) returns a
  `SendError`: `Encoder` when the requested encoder cannot start or failed,
  after the sender refused the encoding to the receiver (`refuse_encoding`),
  else `Connection`. `refuse(reason)` refuses the request for any other
  reason. Nothing switches encoder or encoding on its own.

A new request needs a new connection: the engine reopens the source when the
capture settings change, which closes the connection, and the sender's next
connection (the slim TV worker retries every second) gets the new request.

The slim TV worker takes one Prysm address (`--prysm http://host:8787` or
`https://host[:port]`), resolves its host at every connection attempt, wraps
the sender in an uplink that reconnects at most every second and reports the
wait (the portal answers `503` while no remote source receives: lighting off,
or another capture). It captures at the requested size, rate and menus,
reopening the TV capture when they change, and closes the capture while the
receiver has paused the stream (it keeps reading control messages through
`FrameSink::tick`, so a resume would reach it); a receiver that pauses and
then closes, as on lighting stop, leaves the capture closed until a receiver
connects again. When the requested encoder fails it reports a failure of
kind `encoder` and refuses every later connection with the same request (the
receiver's source fails with the reason, so lighting stops with it in the
portal) until the receiver asks for something else.

To watch a TV's stream on a desktop without lights, run `prysm-headless` with
the `remote` capture and the `null` output; the portal shows the rate and the
preview.

## Wire protocol (version 4)

One WebSocket carries one stream. **The version is the WebSocket
subprotocol** `prysm-capture.v4` ([`CAPTURE_PROTOCOL`](../../crates/ports/src/handoff.rs)):
the sender offers it in `Sec-WebSocket-Protocol`, the portal answers `101`
with it, or `426 Upgrade Required` naming the one it speaks when the sender
offers another (or none). The sender then fails with `WireError::Version {
ours, theirs }` (`theirs` from the `426`'s `Sec-WebSocket-Protocol`; `None`
when the peer names none); the slim TV worker shows it as its waiting reason.

**Text messages** carry JSON, unknown fields refused:

- Receiver → sender, first: the request, `{"type":"request","width":320,
  "height":180,"fps":30,"include_ui":true,"encoding":"h264_hardware"}` (the
  settings' fields; defaults fill omitted ones). It is fixed for the
  connection. A sender that gets anything else first, or nothing within
  3 s, fails with `NoRequest`; one it cannot parse or that fails validation
  with `Json` or `Request`.
- Receiver → sender, any time after: `{"type":"pause"}` (frames are not
  wanted: the sender drops frames unencoded and closes its encoder session,
  releasing a hardware encoder), `{"type":"resume"}` (sends again; the next
  H.264 picture starts a new session), `{"type":"keyframe"}` (the next H.264
  picture starts a new session). The receiver sends pause when its source is
  closed (lighting stops); it never sends resume (the next source's
  connection starts unpaused), but the sender honours it. Anything else
  fails the sender's connection. The sender reads pending messages without
  blocking at the start of each `send` and in `tick`; a receiver that has
  closed shows as `WireError::Closed`.
- Sender → receiver, instead of frames, then the sender closes: the refusal,
  `{"refused":"encoding","reason":"cannot encode frame: hardware H.264
  encoder cannot start: …"}`, `refused` being `encoding` (its encoder cannot
  start or failed) or `request`, the reason at most 1024 bytes. The source
  then fails and stops: an encoding refusal is `Error::Invalid` for field
  `capture.encoding` with the reason `the sender cannot stream 320x180 at 30
  fps with menus as h264_hardware: <reason>`, so the portal can show it on
  that setting; any other refusal is an adapter failure with that context,
  caused by `WireError::Refused(reason)`.

**Binary messages** are frames only (sender → receiver): an 8-byte header,
then the payload; the payload's length is the rest of the message.

| Offset | Size | Field | Value |
| --- | --- | --- | --- |
| 0 | 1 | format | `1` RGB24, `2` BGR24, `3` YUYV, `4` NV12 |
| 1 | 1 | YUV range | `0` (RGB/BGR), `1` limited, `2` full |
| 2 | 1 | YUV matrix | `0` (RGB/BGR), `1` BT.601, `2` BT.709, `3` BT.2020, `4` SMPTE 240, `5` BT.470-6, `6` FCC |
| 3 | 1 | flags | raw: `0`. H.264: bit 0 *keyframe*; other bits `0` |
| 4 | 2 | width | big-endian `u16`, 1..=4096 |
| 6 | 2 | height | big-endian `u16`, 1..=4096 |

Raw: the payload is exactly `width * height * 3` (RGB/BGR), `* 2` (YUYV) or
`* 3 / 2` (NV12) bytes. H.264: 1 to 6,220,800 bytes (1080p RGB). The format
describes the frame's pixels; NV12 is the Y plane (`width` bytes per row)
followed by the interleaved UV plane (`width` bytes per row, `height / 2`
rows), with even width and height. MJPEG and custom YUV matrices are
rejected by the sender. A frame's size is its own: the receiver does not
require the requested one (the slim TV captures at it).

**H.264 stream** (`h264_hardware` or `h264_software`). Each payload is one
access unit (Annex B, Baseline, progressive, one picture) of a frame whose
decoded pixels the header describes: limited-range BT.709 NV12 only, the
webOS capture's native format. An encoder *session* starts with SPS, PPS and
an IDR picture of the current frame, flagged *keyframe*; the receiver
decodes it with a new decoder, so no reference crosses sessions. A session
starts on each connection, after a resume and on a keyframe request; with
the software encoder there are no other IDR pictures, since TCP loses
nothing. Rate control may skip a frame (nothing is sent). Before OpenH264
sees an access unit the receiver checks its NAL types and every SPS (size
equal to the header's, Baseline, progressive, at most 1920x1088,
limited-range BT.709 colour signal), and decodes into a reused `FramePool`
frame, so the processor sees NV12 exactly as from a raw stream. A picture
that does not decode (a lost reference, a corrupt picture, or a stream that
does not start with a keyframe) is dropped, the receiver sends *keyframe*
once and skips pictures until the next keyframe; a keyframe that does not
decode drops the connection.

- **Validation before allocation.** Format, flags, dimensions and the
  payload's length against the header are checked before a frame is
  allocated. The WebSocket bounds each message too: the sender reads at most
  4 KiB, the receiver a header and the largest payload.
- **Malformed input** (a bad header or length, invalid JSON, a keyframe that
  does not decode) drops that connection without panicking, its cause logged
  at debug level; the receiver waits for the sender to connect again.
- **One sender at a time.** A new connection replaces the current one (it
  closes), so a TV that restarts without closing its old socket is accepted
  at once; a second TV takes over from the first, which then reconnects and
  takes over again every second. A sender disconnecting is not an error.
- **Timeouts.** Sender: 3 s for the connection and the upgrade (with TLS),
  3 s for the request, 2 s per write (a receiver that stops reading fails
  the send). Receiver: reads wake every 50 ms to check for `close` and newer
  connections; a quiet connection is kept open.
- **Cost per frame.** 8 bytes of header (16 before version 4) plus the
  WebSocket's 2 to 10 bytes of framing and the client's 4-byte mask key. The
  sender builds each message in one reused buffer and the WebSocket copies it
  into its write buffer, masking it there. The receiver's WebSocket reads it
  into a message buffer; raw pixels are then copied once into a reused
  `FramePool` frame, H.264 is decoded straight from the message.
- **Bandwidth.** Raw: the TV sends its capture tap's native limited BT.709
  NV12 without converting it; 320x180 NV12 at 30 fps is 86,408 bytes per
  frame (header included), about 20.7 Mbit/s; 640x360 about 83 Mbit/s. H.264
  targets 2 Mbit/s (the qualified 640x280 at 30 fps setting).

**Security:** the stream is unauthenticated, like the portal: anyone on the
network can send frames while the remote source receives, or take over its
connection. Browser pages from other sites cannot open it (the portal's
origin policy refuses their `Origin`); the TV's worker sends none. Behind an
HTTPS reverse proxy the stream is encrypted (`wss://`) up to the proxy.

**Certificates:** for `wss://` the sender trusts the system's authorities,
loaded at every connection by tungstenite through `rustls-native-certs`.
`SSL_CERT_FILE` (a PEM bundle) and `SSL_CERT_DIR` replace them when set. On
Linux they are otherwise the first bundle found of
`/etc/ssl/certs/ca-certificates.crt`, `/etc/pki/tls/certs/ca-bundle.crt` and
the other usual paths, plus the files in `/etc/ssl/certs` and
`/etc/pki/tls/certs`; on macOS, the keychains. Finding none fails the
connection with "no native root CA certificates found". Where webOS keeps its
authorities has not been checked on a TV. Tests trust the test CA through
`testkit::in_test_ca_process`; note that Cargo on Linux sets `SSL_CERT_FILE`
and `SSL_CERT_DIR` to the system bundle for the processes it starts.
