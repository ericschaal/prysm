//! The one piece a host wires between the web portal surface and the remote
//! source: both depend on `prysm-ports`, neither on the other, and the
//! hand-over needs nothing beyond `std`.
use std::net::TcpStream;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The portal path a capture-only host (the TV) opens its WebSocket on; the
/// portal hands those connections to the remote source through a [`Handoff`].
pub const CAPTURE_STREAM_PATH: &str = "/api/v1/capture/stream";

/// The capture stream's protocol version, agreed as the WebSocket
/// subprotocol [`CAPTURE_PROTOCOL`] at the upgrade.
pub const CAPTURE_VERSION: u8 = 6;

/// What precedes the version in a capture stream subprotocol.
pub const CAPTURE_PROTOCOL_PREFIX: &str = "prysm-capture.v";

/// The WebSocket subprotocol of the capture stream: both ends offer and
/// accept only this, the portal refuses any other (`426`, listing the
/// versions it speaks).
pub const CAPTURE_PROTOCOL: &str = "prysm-capture.v6";

/// The upgrade request header in which a capture host says what it can
/// deliver, in the source's own format; the surface [announces](Handoff::announce)
/// it whether or not it then hands the connection over.
pub const CAPTURE_OFFER_HEADER: &str = "prysm-capture-offer";

/// The longest announcement kept, in bytes of ASCII.
pub const MAX_ANNOUNCEMENT_BYTES: usize = 4096;

/// A connection a surface accepted and upgraded for a source that reads it
/// itself: the plain, blocking socket and the bytes the surface already read
/// past the upgrade request.
#[derive(Debug)]
pub struct Accepted {
    pub socket: TcpStream,
    pub buffered: Vec<u8>,
}

/// Hands upgraded connections from a surface (the web portal's capture
/// stream path) to the source that reads them (the remote receiver). The
/// host creates one and gives it to both.
///
/// Offers are refused while no source [receives](Self::receive). While one
/// does, the newest connection waits for it to take; it replaces one not yet
/// taken (whose socket closes).
///
/// It also keeps the newest [announcement](Self::announce): what the capture
/// host says it can deliver, opaque here. It outlasts sources and
/// connections, so the receiving source can answer for the capture host
/// while nothing streams; it is not authenticated and never expires.
#[derive(Debug, Clone, Default)]
pub struct Handoff(Arc<Mutex<Slot>>);

#[derive(Debug, Default)]
struct Slot {
    receiving: bool,
    /// Which [`Receiving`] is current: each [`Handoff::receive`] starts a
    /// new one, so an end it replaced neither takes nor ends its offers.
    generation: u64,
    waiting: Option<Accepted>,
    announced: Option<String>,
}

impl Handoff {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Give `connection` to the receiving source.
    ///
    /// # Errors
    /// No source receives: the connection comes back for the surface to close.
    pub fn offer(&self, connection: Accepted) -> Result<(), Accepted> {
        let mut slot = self.lock();
        if !slot.receiving {
            return Err(connection);
        }
        slot.waiting = Some(connection);
        Ok(())
    }

    /// Keep `announcement`, replacing the previous one, whether or not a
    /// source receives. Ignored when longer than [`MAX_ANNOUNCEMENT_BYTES`]
    /// or not ASCII.
    pub fn announce(&self, announcement: &str) {
        if announcement.len() <= MAX_ANNOUNCEMENT_BYTES && announcement.is_ascii() {
            self.lock().announced = Some(announcement.to_owned());
        }
    }

    /// The newest announcement, if any came.
    #[must_use]
    pub fn announcement(&self) -> Option<String> {
        self.lock().announced.clone()
    }

    /// Whether a source receives now; a surface answers the upgrade only
    /// then (one that stops in between closes the offered connection).
    #[must_use]
    pub fn is_receiving(&self) -> bool {
        self.lock().receiving
    }

    /// Accept offers until the returned end is dropped. One source receives
    /// at a time: a later call replaces an end still held, which then takes
    /// nothing and leaves offers open when it drops.
    #[must_use]
    pub fn receive(&self) -> Receiving {
        let mut slot = self.lock();
        slot.receiving = true;
        slot.waiting = None;
        slot.generation = slot.generation.wrapping_add(1);
        Receiving {
            handoff: self.clone(),
            generation: slot.generation,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A source's end of a [`Handoff`]; offers are refused again once the
/// current end drops.
#[derive(Debug)]
pub struct Receiving {
    handoff: Handoff,
    generation: u64,
}

impl Receiving {
    /// The newest connection offered and not yet taken; `None` once a later
    /// [`Handoff::receive`] replaced this end.
    #[must_use]
    pub fn take(&self) -> Option<Accepted> {
        let mut slot = self.handoff.lock();
        if slot.generation == self.generation {
            slot.waiting.take()
        } else {
            None
        }
    }

    /// As [`Handoff::announcement`].
    #[must_use]
    pub fn announcement(&self) -> Option<String> {
        self.handoff.announcement()
    }
}

impl Drop for Receiving {
    fn drop(&mut self) {
        let mut slot = self.handoff.lock();
        if slot.generation == self.generation {
            slot.receiving = false;
            slot.waiting = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    /// A connected socket and its peer, to tell which one arrived.
    fn connection(tag: u8) -> (Accepted, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (socket, _) = listener.accept().unwrap();
        (
            Accepted {
                socket,
                buffered: vec![tag],
            },
            peer,
        )
    }

    #[test]
    fn the_protocol_names_the_version() {
        assert_eq!(
            CAPTURE_PROTOCOL,
            format!("{CAPTURE_PROTOCOL_PREFIX}{CAPTURE_VERSION}")
        );
    }

    #[test]
    fn offers_are_refused_until_a_source_receives() {
        let handoff = Handoff::new();
        let (first, _) = connection(1);
        let refused = handoff.offer(first).unwrap_err();
        assert_eq!(refused.buffered, [1]);

        let receiving = handoff.receive();
        assert!(receiving.take().is_none());
        handoff.offer(refused).unwrap();
        assert_eq!(receiving.take().unwrap().buffered, [1]);
        assert!(receiving.take().is_none());

        drop(receiving);
        let (late, _) = connection(2);
        assert!(handoff.offer(late).is_err());
    }

    /// A source that received before the current one, dropped late, neither
    /// takes the current one's connection nor ends its receiving.
    #[test]
    fn a_replaced_end_leaves_the_current_one_alone() {
        let handoff = Handoff::new();
        let replaced = handoff.receive();
        let current = handoff.receive();
        let (first, _) = connection(1);
        handoff.offer(first).unwrap();
        assert!(replaced.take().is_none());
        drop(replaced);
        assert!(handoff.is_receiving());
        assert_eq!(current.take().unwrap().buffered, [1]);

        let (second, _) = connection(2);
        handoff.offer(second).unwrap();
        drop(current);
        assert!(!handoff.is_receiving());
        let (late, _) = connection(3);
        assert!(handoff.offer(late).is_err());
    }

    #[test]
    fn announcements_outlast_sources_and_are_bounded() {
        let handoff = Handoff::new();
        assert_eq!(handoff.announcement(), None);
        handoff.announce("first");
        let receiving = handoff.receive();
        assert_eq!(receiving.announcement().as_deref(), Some("first"));
        handoff.announce("second");
        drop(receiving);
        assert_eq!(handoff.announcement().as_deref(), Some("second"));
        handoff.announce(&"x".repeat(MAX_ANNOUNCEMENT_BYTES + 1));
        handoff.announce("é");
        assert_eq!(handoff.announcement().as_deref(), Some("second"));
        handoff.announce(&"x".repeat(MAX_ANNOUNCEMENT_BYTES));
        assert_eq!(
            handoff.announcement().map(|a| a.len()),
            Some(MAX_ANNOUNCEMENT_BYTES)
        );
    }

    #[test]
    fn the_newest_connection_wins_and_older_ones_close() {
        let handoff = Handoff::new();
        let receiving = handoff.receive();
        let (first, mut first_peer) = connection(1);
        let (second, _) = connection(2);
        handoff.offer(first).unwrap();
        handoff.offer(second).unwrap();
        // The replaced connection was closed: its peer reads end of stream.
        assert_eq!(first_peer.read(&mut [0; 1]).unwrap(), 0);
        assert_eq!(receiving.take().unwrap().buffered, [2]);

        // One not taken when the source stops is closed too.
        let (third, mut third_peer) = connection(3);
        handoff.offer(third).unwrap();
        drop(receiving);
        assert_eq!(third_peer.read(&mut [0; 1]).unwrap(), 0);
    }
}
