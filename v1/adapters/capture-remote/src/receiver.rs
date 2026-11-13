//! The lighting-host end: a listener thread that keeps only the newest frame.
use crate::request::{StreamEncoding, StreamRequest};
use crate::wire::{self, Control, Encoding, HEADER_LEN, HELLO_LEN, Header, Refused, WireError};
use prysm_core::FramePool;
use prysm_h264::Decoder;
use prysm_ports::{
    Error, FrameSource, OwnerThread, Pull, Result, Section, SourceFactory, ValidationError, Worker,
    chain,
};
use serde::{Deserialize, Serialize};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How often the receive thread checks for `close` and for a newer sender.
/// `close` returns within about one tick.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long `close` waits for the receive thread, which stops within a tick.
const RELEASE: Duration = Duration::from_secs(1);

/// Settings for kind `"remote"`: where to listen and the stream to ask the
/// sender for, `{ "kind": "remote", "listen": "0.0.0.0:7380", "width": 320,
/// "height": 180, "fps": 30, "include_ui": true, "encoding": "h264_hardware" }`.
/// Every field but `listen` is optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct RemoteReceiverSettings {
    /// Address and fixed port the capture host connects to.
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "0.0.0.0:7380"))]
    pub listen: SocketAddr,
    /// Requested picture width: even, 2 to 1920 (H.264: at least 16).
    #[serde(default = "defaults::width")]
    #[cfg_attr(
        feature = "schema",
        schema(minimum = 2, maximum = 1920, multiple_of = 2, default = 320)
    )]
    pub width: u32,
    /// Requested picture height: even, 2 to 1080 (H.264: at least 16).
    #[serde(default = "defaults::height")]
    #[cfg_attr(
        feature = "schema",
        schema(minimum = 2, maximum = 1080, multiple_of = 2, default = 180)
    )]
    pub height: u32,
    /// Requested frames per second.
    #[serde(default = "defaults::fps")]
    #[cfg_attr(feature = "schema", schema(minimum = 1, maximum = 60, default = 30))]
    pub fps: u32,
    /// Capture on-screen menus over the video and, when nothing is playing
    /// (such as the Home screen), the TV's own screens; false: video only.
    #[serde(default = "defaults::include_ui")]
    #[cfg_attr(feature = "schema", schema(default = true))]
    pub include_ui: bool,
    /// How the sender encodes the stream. It uses exactly this encoder; if
    /// that fails, lighting stops with the sender's reason.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schema(default = "h264_hardware"))]
    pub encoding: StreamEncoding,
}

mod defaults {
    use super::StreamRequest;

    pub fn width() -> u32 {
        StreamRequest::default().width
    }
    pub fn height() -> u32 {
        StreamRequest::default().height
    }
    pub fn fps() -> u32 {
        StreamRequest::default().fps
    }
    pub fn include_ui() -> bool {
        StreamRequest::default().include_ui
    }
}

impl RemoteReceiverSettings {
    /// Listen on `listen` and ask for the default stream.
    #[must_use]
    pub fn new(listen: SocketAddr) -> Self {
        Self::with_request(listen, StreamRequest::default())
    }

    #[must_use]
    pub fn with_request(listen: SocketAddr, request: StreamRequest) -> Self {
        let StreamRequest {
            width,
            height,
            fps,
            include_ui,
            encoding,
        } = request;
        Self {
            listen,
            width,
            height,
            fps,
            include_ui,
            encoding,
        }
    }

    /// The stream asked of the sender.
    #[must_use]
    pub fn request(&self) -> StreamRequest {
        StreamRequest {
            width: self.width,
            height: self.height,
            fps: self.fps,
            include_ui: self.include_ui,
            encoding: self.encoding,
        }
    }
}

/// Registers kind `"remote"`: frames sent by a [`RemoteSender`](crate::RemoteSender).
#[derive(Debug, Clone, Copy, Default)]
pub struct RemoteReceiver;

impl SourceFactory for RemoteReceiver {
    const KIND: &'static str = "remote";
    type Settings = RemoteReceiverSettings;

    fn validate(&self, settings: &RemoteReceiverSettings) -> Result<(), ValidationError> {
        if settings.listen.port() == 0 {
            return Err(ValidationError::new(
                "listen",
                "needs a fixed port the sender can connect to",
            ));
        }
        settings.request().validate()
    }

    fn open(&self, settings: &RemoteReceiverSettings) -> Result<Box<dyn FrameSource>> {
        Ok(Box::new(RemoteSource::bind(
            settings.listen,
            settings.request(),
        )?))
    }
}

/// A listening socket and its receive thread. One sender at a time; a new
/// connection replaces the current one. Frames are not queued: `next_frame`
/// returns the newest complete frame received since the last call.
///
/// Every sender is asked for the same [`StreamRequest`]; a new request needs
/// a new source (the engine reopens it when the settings change, and the
/// sender reconnects). A sender that cannot produce the request refuses it:
/// `next_frame` then fails once with its reason, as an invalid
/// `capture.encoding` when the requested encoder is what failed.
#[derive(Debug)]
pub struct RemoteSource {
    local_addr: SocketAddr,
    request: StreamRequest,
    refused: Arc<Mutex<Option<Refusal>>>,
    thread: OwnerThread,
}

impl RemoteSource {
    /// Bind `listen` (port 0 picks a free port; see [`Self::local_addr`])
    /// and ask every sender for `request`.
    ///
    /// # Errors
    /// The address cannot be bound or the receive thread cannot start.
    pub fn bind(listen: SocketAddr, request: StreamRequest) -> Result<Self> {
        let listener = TcpListener::bind(listen)
            .map_err(|error| Error::adapter(format!("cannot listen on {listen}"), error))?;
        let local_addr = listener
            .set_nonblocking(true)
            .and_then(|()| listener.local_addr())
            .map_err(|error| {
                Error::adapter(format!("cannot configure listener {listen}"), error)
            })?;
        let refused = Arc::new(Mutex::new(None));
        let thread =
            OwnerThread::spawn("prysm-remote-receiver", "remote receive thread", RELEASE, {
                let refused = refused.clone();
                move |worker| {
                    Receiver {
                        wake: Wake {
                            listener,
                            worker: worker.clone(),
                        },
                        request,
                        refused,
                        pool: FramePool::default(),
                        rejected: None,
                        payload: Vec::new(),
                    }
                    .run();
                    Ok(())
                }
            })?;
        Ok(Self {
            local_addr,
            request,
            refused,
            thread,
        })
    }

    /// What every sender is asked for.
    #[must_use]
    pub fn request(&self) -> StreamRequest {
        self.request
    }

    /// The bound address, with the actual port.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

/// A sender's refusal: its reason and what it names as the cause.
#[derive(Debug)]
struct Refusal {
    reason: String,
    refused: Refused,
}

impl FrameSource for RemoteSource {
    fn next_frame(&mut self, timeout: Duration) -> Result<Pull> {
        let refused = self
            .refused
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(Refusal { reason, refused }) = refused {
            let context = format!("the sender cannot stream {}", self.request);
            return Err(match refused {
                Refused::Encoding => Error::Invalid(ValidationError::new(
                    format!("{}.encoding", Section::Capture.as_str()),
                    format!("{context}: {reason}"),
                )),
                Refused::Request => Error::adapter(context, WireError::Refused(reason)),
            });
        }
        self.thread.next_frame(timeout)
    }

    fn close(self: Box<Self>) -> Result<()> {
        self.thread.close()
    }
}

/// What the receive thread does after a connection ends.
enum Next {
    Stop,
    Wait,
    Switch(TcpStream),
}

/// The listener and the owner's stop request, checked whenever a read would block.
struct Wake {
    listener: TcpListener,
    worker: Worker,
}

impl Wake {
    fn stopped(&self) -> bool {
        self.worker.stopped()
    }

    /// The newest pending connection, if any.
    fn accept(&self) -> Option<TcpStream> {
        let mut newest = None;
        // `WouldBlock` ends the backlog; other errors (an aborted handshake,
        // descriptor exhaustion) are retried on the next tick.
        while let Ok((stream, _)) = self.listener.accept() {
            // Accepted sockets may inherit non-blocking mode on BSD/macOS.
            if stream
                .set_nonblocking(false)
                .and_then(|()| stream.set_read_timeout(Some(POLL_INTERVAL)))
                .is_ok()
            {
                newest = Some(stream);
            }
        }
        newest
    }

    /// `None` once stopped.
    fn wait_for_sender(&self) -> Option<TcpStream> {
        loop {
            if self.stopped() {
                return None;
            }
            if let Some(stream) = self.accept() {
                return Some(stream);
            }
            self.worker.sleep(POLL_INTERVAL);
        }
    }

    /// Fill `buf`, or say why the connection ended.
    fn read_full(&self, mut stream: &TcpStream, buf: &mut [u8]) -> Result<(), Next> {
        let mut filled = 0;
        while filled < buf.len() {
            match stream.read(&mut buf[filled..]) {
                Ok(0) => return Err(Next::Wait),
                Ok(read) => filled += read,
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                    ) =>
                {
                    if self.stopped() {
                        return Err(Next::Stop);
                    }
                    if let Some(newer) = self.accept() {
                        return Err(Next::Switch(newer));
                    }
                }
                Err(_) => return Err(Next::Wait),
            }
        }
        Ok(())
    }
}

struct Receiver {
    wake: Wake,
    request: StreamRequest,
    /// The sender's refusal of the request, until `next_frame` reports it.
    refused: Arc<Mutex<Option<Refusal>>>,
    pool: FramePool,
    /// The last refused hello, so a sender retrying every second is logged once.
    rejected: Option<WireError>,
    /// The current H.264 access unit, reused (at most `MAX_PAYLOAD_BYTES`).
    payload: Vec<u8>,
}

/// One control byte to the sender. The socket's send buffer is empty in
/// practice (the sender reads every frame), so this does not block; if the
/// sender is gone, the next read ends the connection.
fn send_control(mut stream: &TcpStream, control: Control) {
    let _ = stream.write_all(&[control.code()]);
}

/// Decoding state of one H.264 connection.
struct H264 {
    decoder: Option<Decoder>,
    /// Pictures are skipped until a keyframe; the sender was asked for one.
    waiting: bool,
}

impl Receiver {
    fn run(mut self) {
        let mut next = Next::Wait;
        loop {
            let stream = match next {
                Next::Stop => return,
                Next::Switch(stream) => stream,
                Next::Wait => match self.wake.wait_for_sender() {
                    Some(stream) => stream,
                    None => return,
                },
            };
            next = match self.greet(&stream) {
                Ok(()) => self.serve(&stream, self.request.encoding.into()),
                Err(next) => next,
            };
        }
    }

    /// Read the hello and answer it with the request. A refused hello is
    /// answered with this end's version and closed; the refusal is logged.
    fn greet(&mut self, mut stream: &TcpStream) -> Result<(), Next> {
        let mut hello = [0; HELLO_LEN];
        self.wake.read_full(stream, &mut hello)?;
        let outcome = wire::accept(&hello);
        // The sender learns the outcome from the reply; if it is gone, the
        // next read ends the connection.
        let _ = stream.write_all(&wire::reply(outcome.as_ref().map(|()| &self.request)));
        match outcome {
            Ok(()) => {
                self.rejected = None;
                Ok(())
            }
            Err(error) => {
                if self.rejected.as_ref() != Some(&error) {
                    tracing::warn!(%error, "refused a remote frame stream");
                }
                self.rejected = Some(error);
                Err(Next::Wait)
            }
        }
    }

    /// Receive frames until the connection ends. Malformed input drops the
    /// connection; the sender is expected to reconnect. On `close` the
    /// sender is paused before the connection ends.
    fn serve(&mut self, stream: &TcpStream, encoding: Encoding) -> Next {
        let mut h264 = H264 {
            decoder: None,
            waiting: false,
        };
        let next = loop {
            if self.wake.stopped() {
                break Next::Stop;
            }
            if let Some(newer) = self.wake.accept() {
                break Next::Switch(newer);
            }
            let mut header = [0; HEADER_LEN];
            if let Err(next) = self.wake.read_full(stream, &mut header) {
                break next;
            }
            if let Some(refusal) = wire::refusal_len(&header) {
                if let Ok((len, refused)) = refusal {
                    self.payload.resize(len, 0);
                    let mut reason = std::mem::take(&mut self.payload);
                    let read = self.wake.read_full(stream, &mut reason);
                    if read.is_ok() {
                        self.refuse(Refusal {
                            reason: String::from_utf8_lossy(&reason).into_owned(),
                            refused,
                        });
                    }
                    self.payload = reason;
                }
                break Next::Wait;
            }
            // Validated before allocating.
            let Ok((header, len)) = Header::decode(&header, encoding) else {
                break Next::Wait;
            };
            let Ok(mut writer) = self
                .pool
                .acquire(header.width, header.height, header.format)
            else {
                break Next::Wait;
            };
            if encoding == Encoding::Raw {
                if let Err(next) = self.wake.read_full(stream, writer.as_mut_slice()) {
                    break next;
                }
                self.wake.worker.offer(writer.finish());
                continue;
            }
            self.payload.resize(len, 0);
            let mut payload = std::mem::take(&mut self.payload);
            let read = self.wake.read_full(stream, &mut payload);
            let decoded =
                read.map(|()| h264.decode(stream, &header, &payload, writer.as_mut_slice()));
            self.payload = payload;
            match decoded {
                Err(next) => break next,
                Ok(Ok(true)) => {
                    self.wake.worker.offer(writer.finish());
                }
                Ok(Ok(false)) => {}
                // A keyframe that does not decode: start over on a new connection.
                Ok(Err(())) => break Next::Wait,
            }
        };
        if matches!(next, Next::Stop) {
            send_control(stream, Control::Pause);
        }
        next
    }
}

impl Receiver {
    /// The sender cannot produce the request: keep its reason for `next_frame`.
    fn refuse(&self, refusal: Refusal) {
        tracing::warn!(
            request = %self.request,
            reason = refusal.reason,
            refused = ?refusal.refused,
            "the remote sender refused the stream"
        );
        *self.refused.lock().unwrap_or_else(PoisonError::into_inner) = Some(refusal);
    }
}

impl H264 {
    /// `Ok(true)`: `nv12` holds the picture. `Ok(false)`: skipped while
    /// waiting for a keyframe (one was requested). `Err`: a keyframe failed
    /// to decode, so the stream itself is broken.
    fn decode(
        &mut self,
        stream: &TcpStream,
        header: &Header,
        payload: &[u8],
        nv12: &mut [u8],
    ) -> Result<bool, ()> {
        if header.keyframe {
            // A new encoder session: never carry references across sessions.
            self.decoder = None;
            self.waiting = false;
        } else if self.waiting {
            return Ok(false);
        }
        let decoded = match &mut self.decoder {
            Some(decoder) => decoder.decode(payload, header.width, header.height, nv12),
            None if header.keyframe => Decoder::new().and_then(|decoder| {
                self.decoder
                    .insert(decoder)
                    .decode(payload, header.width, header.height, nv12)
            }),
            // A stream must start with a keyframe.
            None => Err(prysm_h264::H264Error::NoPicture),
        };
        match decoded {
            Ok(()) => Ok(true),
            Err(error) if header.keyframe => {
                tracing::warn!(error = %chain(&error), "a remote H.264 keyframe did not decode");
                Err(())
            }
            Err(error) => {
                tracing::debug!(error = %chain(&error), "remote H.264 picture lost; requesting a keyframe");
                self.decoder = None;
                self.waiting = true;
                send_control(stream, Control::Keyframe);
                Ok(false)
            }
        }
    }
}

#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{Map, Value, json};
    use std::collections::BTreeSet;
    use utoipa::ToSchema;

    /// The schema lists exactly the serialized fields, requires exactly those
    /// serde cannot default, and rejects unknown fields as serde does.
    fn same_shape<T: ToSchema + Serialize + DeserializeOwned>(full: &T) -> Value {
        let schema = serde_json::to_value(T::schema()).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        let required: Vec<String> =
            serde_json::from_value(schema["required"].clone()).unwrap_or_default();
        let Value::Object(fields) = serde_json::to_value(full).unwrap() else {
            panic!("settings serialize as an object");
        };
        let keys = |map: &Map<String, Value>| map.keys().cloned().collect::<BTreeSet<_>>();
        assert_eq!(keys(&fields), keys(properties));
        let minimal: Map<String, Value> = fields
            .iter()
            .filter(|(key, _)| required.contains(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        serde_json::from_value::<T>(minimal.clone().into()).unwrap();
        for key in &required {
            let mut missing = minimal.clone();
            missing.remove(key);
            assert!(
                serde_json::from_value::<T>(missing.into()).is_err(),
                "{key}"
            );
        }
        assert_eq!(schema["additionalProperties"], json!(false));
        let mut unknown = fields;
        unknown.insert("unknown".into(), json!(1));
        assert!(serde_json::from_value::<T>(unknown.into()).is_err());
        schema
    }

    #[test]
    fn schema_matches_serde_shape() {
        let settings = RemoteReceiverSettings::new("0.0.0.0:7380".parse().unwrap());
        let schema = same_shape(&settings);
        assert_eq!(schema["required"], json!(["listen"]));
        assert_eq!(schema["properties"]["listen"]["type"], json!("string"));
        for (field, default) in [
            ("width", json!(320)),
            ("height", json!(180)),
            ("fps", json!(30)),
            ("include_ui", json!(true)),
            ("encoding", json!("h264_hardware")),
        ] {
            assert_eq!(schema["properties"][field]["default"], default, "{field}");
            assert_eq!(serde_json::to_value(settings).unwrap()[field], default);
        }
        assert_eq!(schema["properties"]["fps"]["maximum"], json!(60));
    }
}
