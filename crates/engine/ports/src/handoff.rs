//! The one piece a host wires between the web portal surface and the remote
//! source: both depend on `prysm-ports`, neither on the other, and the
//! hand-over needs nothing beyond `std`.
use crate::Wakeup;
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
/// deliver, in the source's own format; [`Handoff::answer`] announces it
/// whether or not the connection is then handed over.
pub const CAPTURE_OFFER_HEADER: &str = "prysm-capture-offer";

/// The longest offer kept, in bytes of ASCII.
pub const MAX_ANNOUNCEMENT_BYTES: usize = 4096;

/// A connection a surface accepted and upgraded for a source that reads it
/// itself: the plain, blocking socket, the bytes the surface already read
/// past the upgrade request, and the offer that request carried
/// ([`Answer::Upgrade`]).
#[derive(Debug)]
pub struct Accepted {
    pub socket: TcpStream,
    pub buffered: Vec<u8>,
    pub offer: Option<String>,
}

/// How a surface answers an upgrade request at [`CAPTURE_STREAM_PATH`]
/// ([`Handoff::answer`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// `101` with [`CAPTURE_PROTOCOL`], then [offer](Handoff::offer) the
    /// connection as an [`Accepted`] with this offer: the request's
    /// [`CAPTURE_OFFER_HEADER`], `None` when it had none of at most
    /// [`MAX_ANNOUNCEMENT_BYTES`] of ASCII.
    Upgrade { offer: Option<String> },
    /// `426 Upgrade Required`, listing [`CAPTURE_PROTOCOL`]: the request
    /// offers no version spoken here.
    Version,
    /// `503 Service Unavailable`: no source receives.
    Unavailable,
}

/// Hands upgraded connections from a surface (the web portal's capture
/// stream path) to the source that reads them (the remote receiver). The
/// host creates one and gives it to both.
///
/// Offers are refused while no source [receives](Self::receive). While one
/// does, the newest connection waits for it to take; it replaces one not yet
/// taken, whose socket closes without a WebSocket close.
///
/// It also keeps the newest announcement ([`Self::announcement`]): what a
/// capture host said it can deliver at the latest upgrade request, opaque
/// here. It outlasts sources and connections, so the receiving source can
/// answer for the capture host while nothing streams; it is not
/// authenticated and never expires. Each connection carries its own
/// sender's offer ([`Accepted::offer`]).
#[derive(Debug, Clone, Default)]
pub struct Handoff(Arc<Mutex<Slot>>);

#[derive(Debug, Default)]
struct Slot {
    receiving: bool,
    /// Which [`Receiving`] is current: each [`Handoff::receive`] starts a
    /// new one, so an end it replaced neither takes nor ends its offers.
    generation: u64,
    waiting: Option<Accepted>,
    /// Woken by each offer: the current end's thread ([`Receiving::wake`]).
    wakeup: Option<Wakeup>,
    announced: Option<String>,
}

impl Handoff {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Answer an upgrade request whose `Sec-WebSocket-Protocol` header
    /// values are `protocols` (each comma-separated; names are case-sensitive) and whose
    /// [`CAPTURE_OFFER_HEADER`] is `offer`, in this order: a request that
    /// offers no version spoken here is refused; its offer, if at most
    /// [`MAX_ANNOUNCEMENT_BYTES`] of ASCII, is announced, replacing the
    /// previous one, so a refused attempt still says what its sender can
    /// deliver; it is refused while no source receives; else it is
    /// upgraded, carrying the offer. A source that stops before the
    /// connection is [offered](Self::offer) refuses it then.
    pub fn answer<'a>(
        &self,
        protocols: impl IntoIterator<Item = &'a str>,
        offer: Option<&str>,
    ) -> Answer {
        let speaks = protocols
            .into_iter()
            .flat_map(|value| value.split(','))
            .any(|protocol| protocol.trim() == CAPTURE_PROTOCOL);
        if !speaks {
            return Answer::Version;
        }
        let offer = offer
            .filter(|offer| offer.len() <= MAX_ANNOUNCEMENT_BYTES && offer.is_ascii())
            .map(str::to_owned);
        let mut slot = self.lock();
        if let Some(offer) = &offer {
            slot.announced = Some(offer.clone());
        }
        if slot.receiving {
            Answer::Upgrade { offer }
        } else {
            Answer::Unavailable
        }
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
        if let Some(wakeup) = &slot.wakeup {
            wakeup.wake();
        }
        Ok(())
    }

    /// The newest announcement, if any came.
    #[must_use]
    pub fn announcement(&self) -> Option<String> {
        self.lock().announced.clone()
    }

    /// Whether a source receives now.
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
        slot.wakeup = None;
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

    /// Wake `wakeup` at every offer to this end, so its thread waits for one
    /// without polling; nothing once a later [`Handoff::receive`] replaced
    /// this end.
    pub fn wake(&self, wakeup: Wakeup) {
        let mut slot = self.handoff.lock();
        if slot.generation == self.generation {
            slot.wakeup = Some(wakeup);
        }
    }
}

impl Drop for Receiving {
    fn drop(&mut self) {
        let mut slot = self.handoff.lock();
        if slot.generation == self.generation {
            slot.receiving = false;
            slot.waiting = None;
            slot.wakeup = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;
    use std::time::Duration;

    /// A connected socket and its peer, to tell which one arrived.
    fn connection(tag: u8) -> (Accepted, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (socket, _) = listener.accept().unwrap();
        (
            Accepted {
                socket,
                buffered: vec![tag],
                offer: None,
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

    /// Every protocol header x offer x receiving: the version is checked
    /// first, a bounded ASCII offer is announced either way, and only a
    /// receiving source upgrades, carrying the offer.
    #[test]
    fn answers_check_the_version_then_announce_then_receive() {
        let other = "prysm-capture.v5";
        let listed = "prysm-capture.v5, prysm-capture.v6";
        // Subprotocols are case-sensitive: the 101 echoes this end's name,
        // which a client that sent another case would refuse.
        let other_case = "PRYSM-CAPTURE.V6";
        let longest = "x".repeat(MAX_ANNOUNCEMENT_BYTES);
        let too_long = "x".repeat(MAX_ANNOUNCEMENT_BYTES + 1);
        for receiving in [false, true] {
            for (protocols, speaks) in [
                (&[][..], false),
                (&[other][..], false),
                (&[CAPTURE_PROTOCOL][..], true),
                (&[listed][..], true),
                (&[other_case][..], false),
                (&[other, CAPTURE_PROTOCOL][..], true),
            ] {
                for (offer, kept) in [
                    (None, None),
                    (Some("{}"), Some("{}")),
                    (Some(longest.as_str()), Some(longest.as_str())),
                    (Some(too_long.as_str()), None),
                    (Some("é"), None),
                ] {
                    let handoff = Handoff::new();
                    let end = receiving.then(|| handoff.receive());
                    let expected = match (speaks, receiving) {
                        (false, _) => Answer::Version,
                        (true, false) => Answer::Unavailable,
                        (true, true) => Answer::Upgrade {
                            offer: kept.map(str::to_owned),
                        },
                    };
                    let case = (receiving, protocols, offer.map(str::len));
                    assert_eq!(
                        handoff.answer(protocols.iter().copied(), offer),
                        expected,
                        "{case:?}"
                    );
                    let announced = kept.filter(|_| speaks);
                    assert_eq!(handoff.announcement().as_deref(), announced, "{case:?}");
                    drop(end);
                }
            }
        }
    }

    #[test]
    fn announcements_outlast_sources_and_replace_each_other() {
        let handoff = Handoff::new();
        assert_eq!(handoff.announcement(), None);
        let _ = handoff.answer([CAPTURE_PROTOCOL], Some("first"));
        let receiving = handoff.receive();
        assert_eq!(handoff.announcement().as_deref(), Some("first"));
        let _ = handoff.answer([CAPTURE_PROTOCOL], Some("second"));
        drop(receiving);
        assert_eq!(handoff.announcement().as_deref(), Some("second"));
        // A request without one, or with one it does not keep, leaves it.
        let _ = handoff.answer([CAPTURE_PROTOCOL], None);
        let _ = handoff.answer([CAPTURE_PROTOCOL], Some("é"));
        assert_eq!(handoff.announcement().as_deref(), Some("second"));
    }

    /// A thread waiting for a connection sleeps until one is offered.
    #[test]
    fn an_offer_wakes_the_receiving_thread() {
        let handoff = Handoff::new();
        let receiving = handoff.receive();
        let (asleep, sleeping) = std::sync::mpsc::channel();
        let (taken, took) = std::sync::mpsc::channel();
        let owner = crate::OwnerThread::spawn(
            "prysm-test-receiver",
            "test thread",
            Duration::from_secs(5),
            move |worker| {
                receiving.wake(worker.wakeup());
                asleep.send(()).unwrap();
                while !worker.stopped() {
                    if let Some(connection) = receiving.take() {
                        taken.send(connection.buffered).unwrap();
                    }
                    worker.idle();
                }
                Ok(())
            },
        )
        .unwrap();
        sleeping.recv_timeout(Duration::from_secs(5)).unwrap();
        let (first, _peer) = connection(1);
        handoff.offer(first).unwrap();
        assert_eq!(took.recv_timeout(Duration::from_secs(5)).unwrap(), [1]);
        owner.close().unwrap();
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
