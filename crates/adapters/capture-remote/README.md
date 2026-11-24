# prysm-capture-remote

Frames from a capture-only host (webOS slim) to a lighting host (headless)
through the lighting host's web portal. The lighting host decides what it
wants: its settings, `RemoteSettings` (size, rate, menus, encoding), are the
request the sender follows. Both ends are blocking adapters on `std::net`;
neither has a listener of its own:

- `RemoteReceiver` registers source kind `"remote"`:

  ```json
  { "format": 1,
    "capture": { "kind": "remote",
                   "width": 320, "height": 180, "fps": 30,
                   "include_ui": true, "encoding": "h264_hardware" } }
  ```

  Every field is optional; these are the defaults. Like all settings they are
  strict: an unknown field is refused. An even `width` (2 to 1920) and
  `height` (2 to 1080), `fps` 1 to 60 are what the protocol carries; what a
  sender streams is its [offer](#wire-protocol).
  `include_ui` blends the sender's on-screen menus into the picture;
  `encoding` is `h264_hardware` (the TV's encoder), `h264_software`
  (OpenH264 on the TV) or `raw`. The host gives its `incoming()`
  [`Handoff`](../../engine/ports/src/handoff.rs) to the web portal
  (`WebPortal::capture_stream`), which answers the WebSocket upgrade at
  `/api/v1/capture/stream` and hands the connection over as a plain blocking
  socket. `open` takes those connections on one receive thread, which also
  decodes H.264. `next_frame(timeout)` returns the newest complete frame
  (older untaken frames are replaced, never queued) or `Pull::NoPicture` on
  timeout, and fails when the sender's offer excludes the request, the
  sender refuses it, its H.264 stream does not decode or no H.264 decoder
  starts here (see below), once the connection is closed (as *stopped*,
  except for an excluded offer). `close`
  stops the thread within about 50 ms, closes the connection as *stopped*
  and waits up to 1 s for the sender to answer the close.
  `modes(request)` answers from the newest offer a sender announced (refused
  attempts included, so it is known while lighting is off): the modes for
  the request's encoding, none when the sender lacks it, unknown before any
  sender tried. `open` refuses a request that offer excludes, as an invalid
  `capture.width`, `capture.height`, `capture.fps` or `capture.encoding`.
  The announcement is kept for the process's life and is not authenticated.
- `RemoteSender::connect(url, address, offer)` is a `FrameSink`: it opens the
  WebSocket at `url` (`ws://host:port/api/v1/capture/stream`, or `wss://` for
  an `https://` Prysm, verified against the system's certificate
  authorities) by connecting to `address`, where the caller resolved the
  host, sending its `Offer`. It learns the request first (`request()`),
  refusing one it cannot parse, validate or offer, and `send` encodes each
  frame with exactly the requested encoder from
  [`prysm-h264`](../h264/src/lib.rs), opened at the first frame. Any error is
  final for that connection. `deliver` (what `send` calls) returns a
  `SendError`: `Refused` when the requested encoder cannot start or failed,
  or a frame does not fit the stream, after the sender refused the request
  to the receiver; else `Connection`, whose `WireError::Closed` cause names
  the receiver's close code. `refuse(reason)` and `refuse_encoding(reason)`
  refuse the request for the caller's own reasons. Nothing switches encoder
  or encoding on its own. `RemoteSender::connect_until(…, &hangup)` connects
  the same way, given a `prysm_ports::Hangup` (the type a session also
  cuts to end its output's calls); `Hangup::cut` then shuts its socket
  down, which ends a connect (the socket is connecting before a cut can
  reach it), read or write in progress at once, and fails every later
  `connect_until` with that `Hangup`. Every call the cut ends fails with
  the cause `prysm_ports::Stopping`. A host cuts it when it stops.

A new request needs a new connection: the engine reopens the source when the
capture settings change, which closes the connection as *stopped*, and the
sender's next connection (the slim TV worker retries every second) gets the
new request.

The slim TV worker offers its capture's modes (`prysm_capture_webos::MODE`)
raw, and in each H.264 encoder the build has, within that encoder's
limits (`prysm_h264::Backend::limits`): hardware H.264 at most 30 fps and
176×144 to 1920×1080, software from 16×16 to 1920×1080.

The slim TV worker takes one Prysm address (`--prysm http://host:8787` or
`https://host[:port]`), resolves its host at every connection attempt, wraps
the sender in an uplink that reconnects at most every second and reports the
wait (the portal answers `503` while no remote source receives: lighting off,
or another capture). It captures at the requested size, rate and menus,
reopening the TV capture when they change. A connection closed as *stopped*
closes the capture until a receiver takes a connection again (it keeps
reading the receiver's messages through `FrameSink::tick`, so it sees the
close while nothing is captured). A connection closed as *replaced* stops
the uplink: it releases capture and encoder, reports a failure of kind
`replaced` and connects no more until the worker restarts (capture stopped
and started, the TV waking, a new Prysm address, or a restart after a
failure). When the
requested encoder fails it reports a failure of kind `encoder` and refuses
every later connection with the same request (the receiver's source fails
with the reason, so lighting stops with it in the portal) until the receiver
asks for something else.

To watch a TV's stream on a desktop without lights, run `prysm-headless` with
the `remote` capture and the `null` output; the portal shows the rate and the
preview.

## Wire protocol

One WebSocket carries one stream. **The version is the WebSocket
subprotocol**, `prysm-capture.v6`
([`CAPTURE_PROTOCOL`](../../engine/ports/src/handoff.rs)): the sender offers
it in `Sec-WebSocket-Protocol`, the portal answers `101` with it, or
`426 Upgrade Required` listing the versions it speaks (comma-separated) when
the sender offers none of them. The sender then fails with
`WireError::Version { ours, theirs }` (`theirs` the highest version the
`426` lists; `None` when it lists none); the slim TV worker shows it as a
failure of kind `version` and retries every second.

**Versions.** The number rises only when a peer of the previous number can
no longer work with this one: a field removed or its meaning changed, the
frame header changed, or a new message the peer must not ignore. That is a
major Prysm release ([releasing](../../../docs/developers/releasing.md#what-the-version-promises)).
Additions keep the number and are minor: fields, message types, flag bits,
encodings, offer keys, close codes. Every end reads tolerantly, so a peer of
the same number may send what this end does not know:

- unknown fields and message types are ignored;
- an unknown value in a known field: a request's `encoding` is an invalid
  `encoding`, refused as a request; a refusal's `cause` is read as
  `request`; an offer's encoding is skipped;
- what a peer must not ignore is sent only when that peer asked for it. The
  receiver asks the sender for something new only when the sender's offer
  announces it; the sender sends something new only when the request asks
  for it (a request field added later is optional, its absence meaning the
  old behaviour). Features are negotiated this way, not by the version;
- anything else malformed (text that is not JSON, a message missing a
  field, a frame to the sender, a bad frame header) ends the connection.

**The offer** travels in the upgrade request's `prysm-capture-offer` header
([`CAPTURE_OFFER_HEADER`](../../engine/ports/src/handoff.rs)): compact JSON,
the modes the sender streams in each encoding it has (an encoding it lacks
absent) and, optionally, its Prysm release:

```json
{"encodings":{"h264_hardware":[{"width":{"min":176,"max":1920,"step":2},
  "height":{"min":144,"max":1080,"step":2},"fps":{"min":1,"max":30,"step":1}}],
 "raw":[...]},"prysm":"1.0.0"}
```

A mode is every combination of its `width`, `height` and `fps` spans
(`min`, `min + step`, … `max`). At most 4096 ASCII bytes are read;
anything else counts as no offer. The portal announces the header to the
remote source *before* deciding to answer `101` or `503`, so every attempt,
refused or not, delivers it for `modes` and `open`, and hands each
connection over with its own request's offer, against which the request is
checked. A connection whose sender's offer excludes the request is
closed without a request (the sender fails with `NoRequest`), and the source
fails as for an encoding refusal, with the field the offer excludes
(`capture.width`, `capture.height`, `capture.fps`, or `capture.encoding` for
an encoding not offered) and the reason `the sender cannot stream 640x360 at
60 fps with menus as raw: at 640x360 this source delivers a frame rate of 1
to 30`. An unparseable or missing offer leaves the request unchecked; the
sender checks it against its offer too. The receiver logs the sender's
release with a refusal.

**Text messages** carry JSON objects tagged by `type`:

- Receiver → sender, first: the request, `{"type":"request","width":320,
  "height":180,"fps":30,"include_ui":true,"encoding":"h264_hardware"}`, the
  settings' fields, every one required. It is fixed for the connection.
  Messages of other types may come before it and are skipped. A sender that
  gets a frame or a keyframe request first fails with `Unexpected`, one that
  gets no request within the connect's deadline, or a close without a
  code, with `NoRequest`, one closed with a code first with `Closed`. One it cannot parse (`Json`), that fails
  validation or that its offer excludes (`Request`) it refuses, cause
  `request`.
- Receiver → sender, any time after: `{"type":"keyframe"}`, the next H.264
  picture starts a new session. A second request fails the sender's
  connection. The sender reads pending messages without blocking at the
  start of each `send` and in `tick`.
- Sender → receiver, instead of frames, then the sender closes: the refusal,
  `{"type":"refused","cause":"encoding","reason":"cannot encode frame:
  hardware H.264 encoder cannot start: …"}`, `cause` being `encoding` (its
  encoder cannot start or failed, or produced a frame the stream cannot
  carry) or `request` (anything else), the reason at most 1024 bytes. The
  source then fails and stops: an encoding refusal is `Error::Invalid` for
  field `capture.encoding` with the reason `the sender cannot stream 320x180
  at 30 fps with menus as h264_hardware: <reason>`, so the portal can show it
  on that setting; a request refusal is an adapter failure with that
  context, caused by `WireError::Refused(reason)`.

**Close codes** (receiver → sender) say why the receiver ended a stream; the
sender reads them as `WireError::Closed(Some(Close::…))`:

| Code | `Close` | When | The slim TV |
| --- | --- | --- | --- |
| 4000 | `Stopped` | The source closed (lighting stopped, or the capture settings changed); also, before the request, a newer sender that connected while the source closed | Releases capture and encoder, shows *paused*, reconnects every second. Before the request it is a failed connect: the TV shows *waiting* (or keeps *paused*) and retries every second |
| 4001 | `Replaced` | A newer sender's connection replaced this one | Releases capture and encoder, shows a failure of kind `replaced`, connects no more until the worker restarts (capture stopped and started, the TV waking, or a new Prysm address) |
| 4002 | `Failed` | The source failed after its request: the sender refused it, its H.264 keyframe does not decode, or no decoder starts. The source's failure says why | Releases capture and encoder, shows a failure of kind `receiver_failed`, reconnects every second |

Any other close, or none (a crash, the network), is a dropped connection:
the slim TV keeps capturing and reconnects every second. So a sender that
predates a close code (4002 is the newest) reads it as a dropped connection:
an older slim TV keeps its capture and encoder open after a 4002 and
reconnects every second, where a 4000 had released them. It still works with
this receiver (a close code is an addition, see **Versions**), so the
version stays.

**Binary messages** are frames only (sender → receiver): an 8-byte header,
then the payload; the payload's length is the rest of the message.

| Offset | Size | Field | Value |
| --- | --- | --- | --- |
| 0 | 1 | format | `1` RGB24, `2` BGR24, `3` YUYV, `4` NV12 |
| 1 | 1 | YUV range | `0` (RGB/BGR), `1` limited, `2` full |
| 2 | 1 | YUV matrix | `0` (RGB/BGR), `1` BT.601, `2` BT.709, `3` BT.2020, `4` SMPTE 240, `5` BT.470-6, `6` FCC |
| 3 | 1 | flags | H.264: bit 0 *keyframe*. Senders write other bits `0`; receivers ignore them |
| 4 | 2 | width | big-endian `u16`, 1..=1920 |
| 6 | 2 | height | big-endian `u16`, 1..=1080 |

An unknown format, range or matrix code ends the connection. Raw: the payload
is exactly `width * height * 3` (RGB/BGR), `* 2` (YUYV) or `* 3 / 2` (NV12)
bytes. H.264: 1 to 8,388,608 bytes (8 MiB, the largest access unit
`prysm-h264` produces, more than a 1080p RGB frame). The format describes the
frame's pixels; NV12 is the Y plane (`width` bytes per row) followed by the
interleaved UV plane (`width` bytes per row, `height / 2` rows), with even
width and height. Custom YUV matrices are rejected by the sender. A
frame's size is its own: the receiver does not require the requested one (the
slim TV captures at it). A frame the sender cannot put in a message (too
large, or the wrong pixels) is refused, cause `encoding` for an H.264 stream
and `request` for raw.

**H.264 stream** (`h264_hardware` or `h264_software`). Each payload is one
access unit (Annex B, Baseline, progressive, one picture) of a frame whose
decoded pixels the header describes: limited-range BT.709 NV12 only, the
webOS capture's native format. An encoder *session* starts with SPS, PPS and
an IDR picture of the current frame, flagged *keyframe*; the receiver
decodes it with a new decoder, so no reference crosses sessions. A session
starts on each connection and on a keyframe request; with the software
encoder there are no other IDR pictures, since TCP loses nothing. Rate
control may skip a frame (nothing is sent). Before OpenH264 sees an access
unit the receiver checks its NAL types and every SPS (size equal to the
header's, Baseline, progressive, at most 1920x1088, limited-range BT.709
colour signal), and decodes into a reused `FramePool` frame, so the
processor sees NV12 exactly as from a raw stream. A picture that does not
decode (a lost reference, a corrupt picture, or a stream that does not start
with a keyframe) is dropped, the receiver sends *keyframe* once and skips
pictures until the next keyframe; a keyframe request it cannot write drops
the connection. A keyframe that does not decode fails the source at once
with the decoder's error as the cause (the sender would send the same on
every connection).

- **Validation before allocation.** Format, dimensions and the payload's
  length against the header are checked before a frame is allocated. The
  WebSocket bounds each message too: the sender reads at most 4 KiB, the
  receiver a header and the largest payload, text included. Either end
  drops a text message over 4 KiB before parsing it (the receiver after
  reading it whole).
- **Malformed input** (a bad header or length, a message that is not JSON
  or lacks a field) drops that connection without panicking, its cause
  logged at debug level; the receiver waits for the sender to connect again.
- **One sender at a time.** A new connection replaces the current one, which
  is closed as *replaced*, so a TV that restarts without closing its old
  socket is accepted at once. A slim TV that is replaced stops sending, so a
  second TV takes over from the first and keeps the stream. A sender
  disconnecting is not an error. One exception: while the receiver closes
  a connection (up to about a second), connections handed over wait, and
  each replaces the one before it, which closes without a close code; its
  sender sees a dropped connection and reconnects.
- **Timeouts.** Sender: one deadline of 2.25 s for the connection, the
  upgrade (with TLS) and the request, however slowly the receiver answers
  ([`CONNECT_DEADLINE`](src/sender.rs): the receiver's 1 s close write, 1 s
  linger and one 50 ms read for a connection the new one replaces, plus
  200 ms); 2 s for each message sent, however many writes it takes (a
  receiver that reads slowly or not at all fails the send).
  Receiver: a thread without a connection sleeps until one is handed over
  or it is closed; reads wake every 50 ms to check for `close` and newer
  connections; a quiet connection is kept open; 1 s per write. After
  closing a connection it asked for frames as *stopped*, *replaced* or
  *failed* it reads and discards the sender's frames for up to 1 s, until the sender
  answers the close or the connection ends: closing a socket with unread
  frames resets the connection, which could lose the close code (through a
  TLS proxy, say). A `close` during the wait for a replaced sender closes
  the newer connection as *stopped*, without asking it for anything.
- **Cost per frame.** 8 bytes of header plus the WebSocket's 2 to 10 bytes of
  framing and the client's 4-byte mask key. The sender builds each message
  in one reused buffer and the WebSocket copies it into its write buffer,
  masking it there. The receiver's WebSocket reads it into a message buffer;
  raw pixels are then copied once into a reused `FramePool` frame, H.264 is
  decoded straight from the message.
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
loaded through `rustls-native-certs` at the first `wss://` connection and kept
for the process (on macOS loading asks the trust daemon about every keychain
certificate, which takes seconds; verification itself is rustls/webpki in
process, with no revocation or network fetches). A change to the system's
authorities applies after a restart.
`SSL_CERT_FILE` (a PEM bundle) and `SSL_CERT_DIR` replace them when set. On
Linux they are otherwise the first bundle found of
`/etc/ssl/certs/ca-certificates.crt`, `/etc/pki/tls/certs/ca-bundle.crt` and
the other usual paths, plus the files in `/etc/ssl/certs` and
`/etc/pki/tls/certs`; on macOS, the keychains. Finding none fails every
`wss://` connection with "the system has no certificate authorities". Where
webOS keeps its authorities has not been checked on a TV. Tests run
themselves again trusting only the test CA (`testkit::in_test_ca_process`)
or only an unrelated one (`testkit::in_other_ca_process`), so they never read
the system's set; note that Cargo on Linux sets `SSL_CERT_FILE` and
`SSL_CERT_DIR` to the system bundle for the processes it starts.
