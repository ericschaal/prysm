//! The lighting-host end: a listener thread that keeps only the newest frame.
use crate::wire::{self, Encoding, HEADER_LEN, HELLO_LEN, Header, WireError};
use prysm_core::{Frame, FramePool};
use prysm_ports::{Error, FrameSource, LatestSlot, Pull, Result, SourceFactory, ValidationError};
use serde::{Deserialize, Serialize};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// How often the receive thread checks for `close` and for a newer sender.
/// `close` returns within about one tick.
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Settings for kind `"remote"`: `{ "kind": "remote", "listen": "0.0.0.0:7380" }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct RemoteReceiverSettings {
    /// Address and fixed port the capture host connects to.
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "0.0.0.0:7380"))]
    pub listen: SocketAddr,
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
                "capture.listen",
                "needs a fixed port the sender can connect to",
            ));
        }
        Ok(())
    }

    fn open(&self, settings: &RemoteReceiverSettings) -> Result<Box<dyn FrameSource>> {
        Ok(Box::new(RemoteSource::bind(settings.listen)?))
    }
}

/// A listening socket and its receive thread. One sender at a time; a new
/// connection replaces the current one. Frames are not queued: `next_frame`
/// returns the newest complete frame received since the last call.
#[derive(Debug)]
pub struct RemoteSource {
    local_addr: SocketAddr,
    frames: Arc<LatestSlot<Frame>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RemoteSource {
    /// Bind `listen` (port 0 picks a free port; see [`Self::local_addr`]).
    ///
    /// # Errors
    /// The address cannot be bound or the receive thread cannot start.
    pub fn bind(listen: SocketAddr) -> Result<Self> {
        let listener = TcpListener::bind(listen)
            .map_err(|error| Error::adapter(format!("cannot listen on {listen}"), error))?;
        let local_addr = listener
            .set_nonblocking(true)
            .and_then(|()| listener.local_addr())
            .map_err(|error| {
                Error::adapter(format!("cannot configure listener {listen}"), error)
            })?;
        let frames = Arc::new(LatestSlot::new());
        let stop = Arc::new(AtomicBool::new(false));
        let receiver = Receiver {
            wake: Wake {
                listener,
                stop: stop.clone(),
            },
            frames: frames.clone(),
            pool: FramePool::default(),
            rejected: None,
        };
        let thread = thread::Builder::new()
            .name("prysm-remote-receiver".into())
            .spawn(move || receiver.run())
            .map_err(|error| Error::adapter("cannot start the remote receive thread", error))?;
        Ok(Self {
            local_addr,
            frames,
            stop,
            thread: Some(thread),
        })
    }

    /// The bound address, with the actual port.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    fn shutdown(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        match self.thread.take().map(JoinHandle::join) {
            Some(Err(_)) => Err(Error::Panicked("remote receive thread")),
            _ => Ok(()),
        }
    }
}

impl FrameSource for RemoteSource {
    fn next_frame(&mut self, timeout: Duration) -> Result<Pull> {
        if let Some(frame) = self.frames.take_timeout(timeout) {
            return Ok(Pull::Frame(frame));
        }
        // The thread only exits early by panicking.
        if self.thread.as_ref().is_none_or(JoinHandle::is_finished) {
            return Err(Error::Panicked("remote receive thread"));
        }
        Ok(Pull::Idle)
    }

    fn close(mut self: Box<Self>) -> Result<()> {
        self.shutdown()
    }
}

impl Drop for RemoteSource {
    fn drop(&mut self) {
        // Already reported by `close` when it was called.
        let _ = self.shutdown();
    }
}

/// What the receive thread does after a connection ends.
enum Next {
    Stop,
    Wait,
    Switch(TcpStream),
}

/// The listener and stop flag, checked whenever a read would block.
struct Wake {
    listener: TcpListener,
    stop: Arc<AtomicBool>,
}

impl Wake {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
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
            thread::sleep(POLL_INTERVAL);
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
    frames: Arc<LatestSlot<Frame>>,
    pool: FramePool,
    /// The last refused hello, so a sender retrying every second is logged once.
    rejected: Option<WireError>,
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
                Ok(encoding) => self.serve(&stream, encoding),
                Err(next) => next,
            };
        }
    }

    /// Read the hello and answer it. A refused stream is answered with this
    /// end's version and closed; the refusal is logged.
    fn greet(&mut self, mut stream: &TcpStream) -> Result<Encoding, Next> {
        let mut hello = [0; HELLO_LEN];
        self.wake.read_full(stream, &mut hello)?;
        let outcome = wire::accept(&hello);
        // The sender learns the outcome from the reply; if it is gone, the
        // next read ends the connection.
        let _ = stream.write_all(&wire::reply(&outcome));
        match outcome {
            Ok(encoding) => {
                self.rejected = None;
                Ok(encoding)
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
    /// connection; the sender is expected to reconnect.
    fn serve(&mut self, stream: &TcpStream, encoding: Encoding) -> Next {
        loop {
            if self.wake.stopped() {
                return Next::Stop;
            }
            if let Some(newer) = self.wake.accept() {
                return Next::Switch(newer);
            }
            let mut header = [0; HEADER_LEN];
            if let Err(next) = self.wake.read_full(stream, &mut header) {
                return next;
            }
            // Validated before allocating.
            let Ok((header, _)) = Header::decode(&header, encoding) else {
                return Next::Wait;
            };
            let Ok(mut writer) = self
                .pool
                .acquire(header.width, header.height, header.format)
            else {
                return Next::Wait;
            };
            if let Err(next) = self.wake.read_full(stream, writer.as_mut_slice()) {
                return next;
            }
            self.frames.offer(writer.finish());
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
        let settings = RemoteReceiverSettings {
            listen: "0.0.0.0:7380".parse().unwrap(),
        };
        let schema = same_shape(&settings);
        assert_eq!(schema["required"], json!(["listen"]));
        assert_eq!(schema["properties"]["listen"]["type"], json!("string"));
    }
}
