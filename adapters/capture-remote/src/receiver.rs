//! The lighting-host end: the `"remote"` source, fed by the portal.
use crate::receive::Receiver;
use crate::request::StreamRequest;
use prysm_ports::{
    FrameSource, Handoff, OwnerThread, Pull, Result, SourceFactory, ValidationError,
};
use std::time::Duration;

/// How long `close` waits for the receive thread, which stops within a tick.
const RELEASE: Duration = Duration::from_secs(1);

/// Registers kind `"remote"`, its settings the [`StreamRequest`]: frames a
/// [`RemoteSender`](crate::RemoteSender) streams to the portal's capture
/// stream path. It owns the [`Handoff`] those connections come in through;
/// the host gives [`Self::incoming`] to the portal.
#[derive(Debug, Clone, Default)]
pub struct RemoteReceiver {
    incoming: Handoff,
}

impl RemoteReceiver {
    /// Where the portal hands the senders' connections to this source.
    #[must_use]
    pub fn incoming(&self) -> Handoff {
        self.incoming.clone()
    }
}

impl SourceFactory for RemoteReceiver {
    const KIND: &'static str = "remote";
    type Settings = StreamRequest;

    fn validate(&self, request: &StreamRequest) -> Result<(), ValidationError> {
        request.validate()
    }

    fn open(&self, request: &StreamRequest) -> Result<Box<dyn FrameSource>> {
        Ok(Box::new(RemoteSource::open(&self.incoming, *request)?))
    }
}

/// Receives the connections handed over to it, on a receive thread. One
/// sender at a time; a new connection replaces the current one. Frames are
/// not queued: `next_frame` returns the newest complete frame received since
/// the last call.
///
/// Every sender is asked for the same [`StreamRequest`]; a new request needs
/// a new source (the engine reopens it when the settings change, and the
/// sender reconnects). A sender that cannot produce the request refuses it:
/// `next_frame` then fails with its reason, as an invalid `capture.encoding`
/// when the requested encoder is what failed, and the source stops.
#[derive(Debug)]
pub struct RemoteSource {
    request: StreamRequest,
    thread: OwnerThread,
}

impl RemoteSource {
    /// Receive the connections `handoff` is offered, until closed, and ask
    /// every sender for `request`.
    ///
    /// # Errors
    /// The receive thread cannot start.
    pub fn open(handoff: &Handoff, request: StreamRequest) -> Result<Self> {
        let receiving = handoff.receive();
        let thread = OwnerThread::spawn(
            "prysm-remote-receiver",
            "remote receive thread",
            RELEASE,
            move |worker| {
                Receiver::new(receiving, worker.clone(), request).run();
                Ok(())
            },
        )?;
        Ok(Self { request, thread })
    }

    /// What every sender is asked for.
    #[must_use]
    pub fn request(&self) -> StreamRequest {
        self.request
    }
}

impl FrameSource for RemoteSource {
    fn next_frame(&mut self, timeout: Duration) -> Result<Pull> {
        self.thread.next_frame(timeout)
    }

    fn close(self: Box<Self>) -> Result<()> {
        self.thread.close()
    }
}
