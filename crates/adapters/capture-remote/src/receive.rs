//! The receive thread: takes the connections the portal hands over, greets
//! each sender with the request and keeps only the newest frame.
use crate::link::{self, Link, LinkError, Message};
use crate::offer::Offer;
use crate::request::RemoteSettings;
use crate::wire::{
    self, Close, Encoding, FrameMessage, FromSender, HEADER_LEN, Header, MAX_PAYLOAD_BYTES,
    Refusal, RefusalCause, ToSender, WireError,
};
use prysm_core::FramePool;
use prysm_h264::{Decoder, H264Error};
use prysm_ports::{Accepted, Error, Receiving, Section, ValidationError, Worker, chain};
use std::io;
use std::net::TcpStream;
use std::time::Duration;
use tungstenite::WebSocket;
use tungstenite::protocol::Role;

/// How often the receive thread, while it serves a connection, checks for
/// `close` and for a newer sender.
/// The thread leaves a connection within about one tick of `close`.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long one write to the sender (the request, a keyframe request, a
/// close) may block.
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// How long, after closing a connection it asked for frames, the receive
/// thread waits for the sender to answer the close, discarding its frames.
pub(crate) const CLOSE_LINGER: Duration = Duration::from_secs(1);

pub(crate) struct Receiver {
    receiving: Receiving,
    worker: Worker,
    request: RemoteSettings,
    pool: FramePool,
}

/// Why the receive thread leaves a connection.
#[derive(Debug)]
enum Leave {
    /// The connection closed or failed, or a write to it failed.
    SenderGone(LinkError),
    /// The sender sent what this end does not take. Both ends agreed the
    /// version at the upgrade, so the same sender would send it again: the
    /// source fails.
    Malformed(WireError),
    /// A newer sender connected; it replaces this one.
    Replaced(Accepted),
    /// The source closes.
    Stopped,
    /// The source closed before this sender was asked for anything (while
    /// the connection it replaced was being closed).
    StoppedUnasked,
    /// The source fails after asking: the sender refused the request, its
    /// H.264 stream does not decode, or no decoder starts.
    Failed(Error),
    /// The source fails without asking: the sender's offer excludes the
    /// request.
    OfferExcluded(Error),
}

/// Where the receive thread goes after leaving a connection.
#[derive(Debug)]
enum Next {
    /// Wait for a sender to connect.
    Wait,
    /// Serve the newer sender.
    Switch(Accepted),
    /// The thread ends; the source fails with the error, if one is given.
    Stop(Option<Error>),
}

impl Leave {
    /// Finish the wire exchange before handing the receive loop its next action.
    fn finish(self, link: &mut Link<TcpStream>) -> Next {
        let (close, asked, next) = match self {
            Self::SenderGone(cause) => return sender_gone(&cause),
            Self::Malformed(cause) => {
                let error = Error::adapter("the sender sent what Prysm does not take", cause);
                tracing::warn!(error = %chain(&error), "remote stream failed");
                (Close::Failed, true, Next::Stop(Some(error)))
            }
            Self::Replaced(newer) => (Close::Replaced, true, Next::Switch(newer)),
            Self::Stopped => (Close::Stopped, true, Next::Stop(None)),
            Self::StoppedUnasked => (Close::Stopped, false, Next::Stop(None)),
            Self::Failed(error) => (Close::Failed, true, Next::Stop(Some(error))),
            Self::OfferExcluded(error) => (Close::Failed, false, Next::Stop(Some(error))),
        };
        // An asked sender may still have frames in flight. Drain them so
        // dropping the socket doesn't reset it before the close is received.
        if link.close(Some(close)) && asked {
            link.drain(CLOSE_LINGER);
        }
        next
    }
}

fn sender_gone(cause: &LinkError) -> Next {
    tracing::debug!(cause = %chain(cause), "dropped the remote sender's connection");
    Next::Wait
}

/// What became of one H.264 picture.
#[derive(Debug)]
enum Decoded {
    /// The frame holds it.
    Picture,
    /// Skipped while waiting for the keyframe already asked for.
    Skipped,
    /// The stream did not start with a keyframe: ask the sender for one.
    Unstarted,
    /// A picture was lost: ask the sender for a keyframe.
    Lost(H264Error),
    /// A keyframe did not decode: the stream itself is broken.
    Broken(H264Error),
    /// No decoder starts here.
    NoDecoder(H264Error),
}

impl Receiver {
    pub(crate) fn new(receiving: Receiving, worker: Worker, request: RemoteSettings) -> Self {
        Self {
            receiving,
            worker,
            request,
            pool: FramePool::default(),
        }
    }

    /// Serve one connection after another until stopped.
    pub(crate) fn run(mut self) {
        let mut next = Next::Wait;
        loop {
            let connection = match next {
                Next::Wait => match self.wait_for_sender() {
                    Some(connection) => connection,
                    None => return,
                },
                Next::Switch(connection) => connection,
                Next::Stop(failure) => {
                    if let Some(error) = failure {
                        self.worker.fail(error);
                    }
                    return;
                }
            };
            next = self.serve(connection);
        }
    }

    /// The newest connection handed over, waiting for one (each offer wakes
    /// this thread); `None` once stopped. Without end: a sender comes at a
    /// time nothing announces (the TV is switched on, or its capture
    /// started).
    fn wait_for_sender(&self) -> Option<Accepted> {
        loop {
            if self.worker.stopped() {
                return None;
            }
            if let Some(connection) = self.receiving.take() {
                return Some(connection);
            }
            self.worker.idle();
        }
    }

    /// Serve `connection` until leaving it, end it as the reason says and
    /// say where to go next.
    fn serve(&mut self, connection: Accepted) -> Next {
        let Accepted {
            socket,
            buffered,
            offer,
        } = connection;
        let offer = offer.as_deref().and_then(Offer::parse);
        match link_on(socket, buffered) {
            Ok(mut link) => self.greet(&mut link, offer.as_ref()).finish(&mut link),
            Err(error) => sender_gone(&LinkError::Socket(error.into())),
        }
    }

    /// Ask the sender for the request, unless the source stopped or the
    /// sender's own offer excludes the request (one without a readable
    /// offer is asked), then receive its frames. The portal agreed the
    /// version at the upgrade.
    fn greet(&mut self, link: &mut Link<TcpStream>, offer: Option<&Offer>) -> Leave {
        if self.worker.stopped() {
            return Leave::StoppedUnasked;
        }
        if let Some(Err(excluded)) = offer.map(|offer| offer.check(&self.request)) {
            return Leave::OfferExcluded(self.excluded(&excluded, offer));
        }
        if let Err(error) = send(link, &ToSender::Request(self.request)) {
            return Leave::SenderGone(error);
        }
        self.receive(link, offer)
    }

    /// Receive frames until there is a reason to leave. Malformed input, a
    /// refusal or a broken H.264 stream fails the source.
    fn receive(&mut self, link: &mut Link<TcpStream>, offer: Option<&Offer>) -> Leave {
        let encoding = self.request.encoding.into();
        let mut h264 = H264::Fresh;
        loop {
            let message = match self.next_message(link) {
                Ok(Message::Binary(message)) => message,
                Ok(Message::Text(text)) => match wire::decode_from_sender(&text) {
                    Ok(FromSender::Refused(refusal)) => {
                        return Leave::Failed(self.refused(refusal, offer));
                    }
                    Ok(FromSender::Unknown) => continue,
                    Err(error) => return Leave::Malformed(error),
                },
                Err(leave) => return leave,
            };
            let FrameMessage { header, payload } = match wire::decode_frame(&message, encoding) {
                Ok(frame) => frame,
                Err(error) => return Leave::Malformed(error),
            };
            let mut writer = match self
                .pool
                .acquire(header.width, header.height, header.format)
            {
                Ok(writer) => writer,
                Err(error) => return Leave::Malformed(WireError::Geometry(error)),
            };
            let pixels = writer.as_mut_slice();
            match encoding {
                Encoding::Raw => {
                    // `decode_frame` checked the length against the header,
                    // which the pool allocated for; checked again so network
                    // input cannot panic here.
                    if pixels.len() != payload.len() {
                        return Leave::Malformed(WireError::Length {
                            expected: pixels.len(),
                            actual: payload.len(),
                        });
                    }
                    pixels.copy_from_slice(payload);
                }
                Encoding::H264 => match h264.decode(&header, payload, pixels) {
                    Decoded::Picture => {}
                    Decoded::Skipped => continue,
                    // A keyframe request converges: a keyframe resets the
                    // decoder's references, and one that does not decode
                    // fails the source (`Broken`).
                    Decoded::Unstarted => {
                        if let Err(error) = send(link, &ToSender::Keyframe) {
                            return Leave::SenderGone(error);
                        }
                        continue;
                    }
                    Decoded::Lost(error) => {
                        tracing::debug!(error = %chain(&error), "remote H.264 picture lost; requesting a keyframe");
                        // Pictures are skipped until the keyframe: one that
                        // is not asked for would never come.
                        if let Err(error) = send(link, &ToSender::Keyframe) {
                            return Leave::SenderGone(error);
                        }
                        continue;
                    }
                    Decoded::Broken(error) => return Leave::Failed(self.broken(error, offer)),
                    Decoded::NoDecoder(error) => return Leave::Failed(self.no_decoder(error)),
                },
            }
            self.worker.offer(writer.finish());
        }
    }

    /// The next message, unless the source stops or a newer sender connects
    /// first. Stopping comes first: a newer connection still waiting then
    /// closes with the source's end of the hand-over, without a close code.
    fn next_message(&self, link: &mut Link<TcpStream>) -> Result<Message, Leave> {
        loop {
            if self.worker.stopped() {
                return Err(Leave::Stopped);
            }
            if let Some(newer) = self.receiving.take() {
                return Err(Leave::Replaced(newer));
            }
            match link.receive() {
                Ok(Some(message)) => return Ok(message),
                Ok(None) => {}
                Err(LinkError::Wire(error)) if !matches!(error, WireError::Closed(_)) => {
                    return Err(Leave::Malformed(error));
                }
                Err(error) => return Err(Leave::SenderGone(error)),
            }
        }
    }

    /// The sender's offer excludes the request: the source fails with the
    /// field it excludes, as for a sender's refusal of the encoding.
    fn excluded(&self, excluded: &ValidationError, offer: Option<&Offer>) -> Error {
        tracing::warn!(
            request = %self.request,
            field = excluded.field,
            reason = excluded.reason,
            sender = release(offer),
            "the remote sender does not offer the stream"
        );
        Error::Invalid(ValidationError::new(
            format!("{}.{}", Section::Capture.as_str(), excluded.field),
            format!(
                "the sender cannot stream {}: {}",
                self.request, excluded.reason
            ),
        ))
    }

    /// The sender cannot produce the request: the source fails with its
    /// reason, as an invalid `capture.encoding` when the encoder is the
    /// cause.
    fn refused(&self, refusal: Refusal, offer: Option<&Offer>) -> Error {
        tracing::warn!(
            request = %self.request,
            reason = refusal.reason,
            cause = ?refusal.cause,
            sender = release(offer),
            "the remote sender refused the stream"
        );
        let context = format!("the sender cannot stream {}", self.request);
        match refusal.cause {
            RefusalCause::Encoding => Error::Invalid(ValidationError::new(
                format!("{}.encoding", Section::Capture.as_str()),
                format!("{context}: {}", refusal.reason),
            )),
            RefusalCause::Request => Error::adapter(context, WireError::Refused(refusal.reason)),
        }
    }

    /// The sender's H.264 stream does not decode from a keyframe: it would
    /// send the same on every connection, so the source fails with the
    /// decoder's error.
    fn broken(&self, error: H264Error, offer: Option<&Offer>) -> Error {
        let context = format!("the sender's stream ({}) does not decode", self.request);
        tracing::warn!(error = %chain(&error), sender = release(offer), "{context}");
        Error::adapter(context, error)
    }

    /// No H.264 decoder starts on this host.
    fn no_decoder(&self, error: H264Error) -> Error {
        let context = format!("cannot decode the sender's stream ({})", self.request);
        tracing::warn!(error = %chain(&error), "{context}");
        Error::adapter(context, error)
    }
}

/// The sender's Prysm release, from its offer, for logs.
fn release(offer: Option<&Offer>) -> &str {
    offer.and_then(Offer::prysm).unwrap_or("unknown")
}

/// The link on a handed-over socket: its reads wake every
/// [`POLL_INTERVAL`].
fn link_on(socket: TcpStream, buffered: Vec<u8>) -> io::Result<Link<TcpStream>> {
    socket.set_read_timeout(Some(POLL_INTERVAL))?;
    socket.set_write_timeout(Some(WRITE_TIMEOUT))?;
    socket.set_nodelay(true)?;
    // A frame message is the longest this end reads.
    let config = link::config(HEADER_LEN + MAX_PAYLOAD_BYTES);
    Ok(Link::new(WebSocket::from_partially_read(
        socket,
        buffered,
        Role::Server,
        Some(config),
    )))
}

/// One message to the sender, within the write timeout.
fn send(link: &mut Link<TcpStream>, message: &ToSender) -> Result<(), LinkError> {
    link.send(Message::Text(wire::encode_to_sender(message).into()))
}

/// Decoding state of one H.264 connection.
#[derive(Debug)]
enum H264 {
    /// No picture yet.
    Fresh,
    /// Decoding the session its decoder started with.
    Decoding(Decoder),
    /// A picture came before any keyframe or did not decode: pictures are
    /// skipped until a keyframe.
    AwaitingKeyframe,
}

impl H264 {
    /// Decode one picture into `nv12`. A keyframe starts a new decoder
    /// session in any state; a picture that does not decode, or a stream
    /// that does not start with a keyframe, skips pictures until the next
    /// keyframe.
    fn decode(&mut self, header: &Header, payload: &[u8], nv12: &mut [u8]) -> Decoded {
        let (width, height) = (header.width, header.height);
        match (&mut *self, header.keyframe) {
            // A new encoder session: never carry references across sessions.
            (_, true) => {
                let mut decoder = match Decoder::new() {
                    Ok(decoder) => decoder,
                    Err(error) => return Decoded::NoDecoder(error),
                };
                match decoder.decode(payload, width, height, nv12) {
                    Ok(()) => {
                        *self = Self::Decoding(decoder);
                        Decoded::Picture
                    }
                    Err(error) => {
                        *self = Self::AwaitingKeyframe;
                        Decoded::Broken(error)
                    }
                }
            }
            (Self::Fresh, false) => {
                *self = Self::AwaitingKeyframe;
                Decoded::Unstarted
            }
            (Self::AwaitingKeyframe, false) => Decoded::Skipped,
            (Self::Decoding(decoder), false) => {
                match decoder.decode(payload, width, height, nv12) {
                    Ok(()) => Decoded::Picture,
                    Err(error) => {
                        *self = Self::AwaitingKeyframe;
                        Decoded::Lost(error)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::Frame;
    use prysm_h264::{Backend, Encoder, EncoderSettings, FORMAT};
    /// A keyframe, then a picture of its session, of a real encoder.
    fn session(width: u32, height: u32) -> (Vec<u8>, Vec<u8>) {
        let mut encoder = Encoder::new(EncoderSettings::new(Backend::Software, 30)).unwrap();
        let frame = |shift: usize| {
            let (w, h) = (width as usize, height as usize);
            let data = (0..w * h * 3 / 2)
                .map(|i| {
                    if i < w * h {
                        16 + ((i + shift) % 200) as u8
                    } else {
                        128
                    }
                })
                .collect();
            Frame::new(data, width, height, FORMAT).unwrap()
        };
        let keyframe = encoder.encode(&frame(0)).unwrap().unwrap();
        assert!(keyframe.keyframe);
        let keyframe = keyframe.data.to_vec();
        let picture = encoder.encode(&frame(7)).unwrap().unwrap();
        assert!(!picture.keyframe);
        (keyframe, picture.data.to_vec())
    }

    /// A decoding state, without its decoder.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        Fresh,
        Decoding,
        Awaiting,
    }

    fn state(h264: &H264) -> State {
        match h264 {
            H264::Fresh => State::Fresh,
            H264::Decoding(_) => State::Decoding,
            H264::AwaitingKeyframe => State::Awaiting,
        }
    }

    /// An outcome, without its error.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Outcome {
        Picture,
        Skipped,
        Unstarted,
        Lost,
        Broken,
    }

    fn outcome(decoded: &Decoded) -> Outcome {
        match decoded {
            Decoded::Picture => Outcome::Picture,
            Decoded::Skipped => Outcome::Skipped,
            Decoded::Unstarted => Outcome::Unstarted,
            Decoded::Lost(_) => Outcome::Lost,
            Decoded::Broken(_) => Outcome::Broken,
            Decoded::NoDecoder(error) => panic!("no decoder: {error}"),
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum Event {
        Keyframe,
        BrokenKeyframe,
        Picture,
        BrokenPicture,
    }

    /// Every decoding state x {a keyframe that decodes, one that does not, a
    /// picture that decodes, one that does not}: the outcome and the state
    /// after.
    #[test]
    fn every_h264_state_and_picture() {
        let (width, height) = (64, 32);
        let (keyframe, picture) = session(width, height);
        let broken_keyframe = b"\x00\x00\x01\x65\x00".to_vec();
        let broken_picture = b"\x00\x00\x01\x41\x00".to_vec();
        let mut nv12 = vec![0; (width * height * 3 / 2) as usize];
        let header = |keyframe| Header {
            width,
            height,
            format: FORMAT,
            keyframe,
        };
        let event = |event| match event {
            Event::Keyframe => (true, &keyframe),
            Event::BrokenKeyframe => (true, &broken_keyframe),
            Event::Picture => (false, &picture),
            Event::BrokenPicture => (false, &broken_picture),
        };
        let reach = |from, nv12: &mut [u8]| {
            let mut h264 = H264::Fresh;
            match from {
                State::Fresh => {}
                State::Decoding => {
                    h264.decode(&header(true), &keyframe, nv12);
                }
                State::Awaiting => {
                    h264.decode(&header(false), &picture, nv12);
                }
            }
            assert_eq!(state(&h264), from);
            h264
        };
        use {Event as E, Outcome as O, State as S};
        let table = [
            // (state, event) -> (outcome, state after)
            (S::Fresh, E::Keyframe, O::Picture, S::Decoding),
            (S::Fresh, E::BrokenKeyframe, O::Broken, S::Awaiting),
            (S::Fresh, E::Picture, O::Unstarted, S::Awaiting),
            (S::Fresh, E::BrokenPicture, O::Unstarted, S::Awaiting),
            (S::Decoding, E::Keyframe, O::Picture, S::Decoding),
            (S::Decoding, E::BrokenKeyframe, O::Broken, S::Awaiting),
            (S::Decoding, E::Picture, O::Picture, S::Decoding),
            (S::Decoding, E::BrokenPicture, O::Lost, S::Awaiting),
            (S::Awaiting, E::Keyframe, O::Picture, S::Decoding),
            (S::Awaiting, E::BrokenKeyframe, O::Broken, S::Awaiting),
            (S::Awaiting, E::Picture, O::Skipped, S::Awaiting),
            (S::Awaiting, E::BrokenPicture, O::Skipped, S::Awaiting),
        ];
        for (from, on, expected, after) in table {
            let (is_keyframe, payload) = event(on);
            let mut h264 = reach(from, &mut nv12);
            let decoded = h264.decode(&header(is_keyframe), payload, &mut nv12);
            assert_eq!(
                (outcome(&decoded), state(&h264)),
                (expected, after),
                "{from:?} x {on:?}: {decoded:?}"
            );
        }
    }
}
