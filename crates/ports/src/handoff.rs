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
pub const CAPTURE_VERSION: u8 = 4;

/// The WebSocket subprotocol of the capture stream: both ends offer and
/// accept only this, the portal refuses any other (`426`).
pub const CAPTURE_PROTOCOL: &str = "prysm-capture.v4";

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
#[derive(Debug, Clone, Default)]
pub struct Handoff(Arc<Mutex<Slot>>);

#[derive(Debug, Default)]
struct Slot {
    receiving: bool,
    waiting: Option<Accepted>,
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

    /// Whether a source receives now; a surface answers the upgrade only
    /// then (one that stops in between closes the offered connection).
    #[must_use]
    pub fn is_receiving(&self) -> bool {
        self.lock().receiving
    }

    /// Accept offers until the returned end is dropped. One source receives
    /// at a time.
    #[must_use]
    pub fn receive(&self) -> Receiving {
        let mut slot = self.lock();
        slot.receiving = true;
        slot.waiting = None;
        Receiving(self.clone())
    }

    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A source's end of a [`Handoff`]; offers are refused again once it drops.
#[derive(Debug)]
pub struct Receiving(Handoff);

impl Receiving {
    /// The newest connection offered and not yet taken.
    #[must_use]
    pub fn take(&self) -> Option<Accepted> {
        self.0.lock().waiting.take()
    }
}

impl Drop for Receiving {
    fn drop(&mut self) {
        let mut slot = self.0.lock();
        slot.receiving = false;
        slot.waiting = None;
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
            format!("prysm-capture.v{CAPTURE_VERSION}")
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
