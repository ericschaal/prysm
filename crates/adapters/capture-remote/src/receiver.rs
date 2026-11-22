//! The lighting-host end: the `"remote"` source, fed by the portal.
use crate::offer::Offer;
use crate::receive::{CLOSE_LINGER, POLL_INTERVAL, Receiver, WRITE_TIMEOUT};
use crate::request::RemoteSettings;
use prysm_ports::{
    FrameSource, Handoff, Modes, OwnerThread, Pull, Result, SourceFactory, ValidationError,
};
use std::time::Duration;

/// How long `close` waits for the receive thread, in the worst case (`W`
/// the write timeout, `L` the linger, `P` the poll interval; a close is a
/// write and its flush, `2W`; the linger follows only a close that went out
/// and ends within `L + P`):
///
/// - receiving: the read wakes (`P`), close (`2W`), linger (`L + P`):
///   `2W + L + 2P`, 3.1 s;
/// - writing the request or a keyframe request: `W`, then as above
///   without the first wake: `3W + L + P`, 4.05 s;
/// - closing a replaced connection: its close (`2W`) and linger (`L + P`),
///   then the newer connection's close, without a linger (`2W`):
///   `4W + L + P`, 5.05 s, the longest.
const RELEASE: Duration = POLL_INTERVAL
    .saturating_add(WRITE_TIMEOUT.saturating_mul(4))
    .saturating_add(CLOSE_LINGER);

/// The longest a remote source's `close` takes: [`RELEASE`].
pub const RELEASE_LIMIT: Duration = RELEASE;

/// Registers kind `"remote"`, its settings the [`RemoteSettings`]: frames a
/// [`RemoteSender`](crate::RemoteSender) streams to the portal's capture
/// stream path. It owns the [`Handoff`] those connections come in through;
/// the host gives [`Self::incoming`] to the portal, which also announces
/// there each sender's [`Offer`], refused connections included.
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
    type Settings = RemoteSettings;

    fn validate(&self, request: &RemoteSettings) -> Result<(), ValidationError> {
        request.validate()
    }

    /// Refuses a request the newest offer excludes, then receives.
    fn open(&self, request: &RemoteSettings) -> Result<Box<dyn FrameSource>> {
        if let Some(offer) = offered(&self.incoming) {
            offer.check(request)?;
        }
        Ok(Box::new(RemoteSource::open(&self.incoming, *request)?))
    }

    /// What the newest offer streams in the request's encoding; unknown
    /// before a sender has tried to connect.
    fn modes(&self, request: &RemoteSettings) -> Modes {
        offered(&self.incoming).map_or(Modes::Unknown, |offer| offer.modes(request.encoding))
    }
}

/// The newest offer announced at `handoff`, if it is one.
fn offered(handoff: &Handoff) -> Option<Offer> {
    Offer::parse(&handoff.announcement()?)
}

/// Receives the connections handed over to it, on a receive thread. One
/// sender at a time; a new connection replaces the current one. Frames are
/// not queued: `next_frame` returns the newest complete frame received since
/// the last call.
///
/// Every sender is asked for the same [`RemoteSettings`]; a new request needs
/// a new source (the engine reopens it when the settings change, and the
/// sender reconnects). A sender whose [`Offer`] excludes the request is not
/// asked: `next_frame` fails with the invalid field (`capture.width`,
/// `capture.encoding`, ...) and the source stops. So does a sender that
/// cannot produce the request and refuses it, as an invalid
/// `capture.encoding` when the requested encoder is what failed, and an
/// H.264 stream whose keyframe does not decode.
///
/// Closing, or failing after the sender was asked, tells the connected
/// sender that the stream stopped; a newer sender's connection tells the one
/// it replaces.
#[derive(Debug)]
pub struct RemoteSource {
    thread: OwnerThread,
}

impl RemoteSource {
    /// Receive the connections `handoff` is offered, until closed, and ask
    /// every sender for `request`.
    ///
    /// # Errors
    /// The receive thread cannot start.
    pub(crate) fn open(handoff: &Handoff, request: RemoteSettings) -> Result<Self> {
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
        Ok(Self { thread })
    }

    /// The frame received and not yet taken, for tests that wait for one.
    #[cfg(feature = "testkit")]
    #[must_use]
    pub fn pending_frame(&self) -> Option<prysm_core::Frame> {
        self.thread.peek()
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
