//! One WebSocket, either end: text messages (JSON commands and refusals)
//! and binary messages (frames). Of the WebSocket's types the ends see only
//! its text, [`Utf8Bytes`], a string over [`Bytes`].
use crate::wire::{Close, MAX_TEXT_BYTES, WireError};
use bytes::Bytes;
use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::protocol::frame::CloseFrame;
use tungstenite::{Utf8Bytes, WebSocket};

/// One message, from the peer or to it.
pub(crate) enum Message {
    Text(Utf8Bytes),
    Binary(Bytes),
}

/// Why a link ended.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LinkError {
    /// The peer closed it (saying why, when it is a [`Close`]), or sent
    /// what this end does not take.
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error(transparent)]
    Socket(#[from] tungstenite::Error),
}

impl LinkError {
    /// An adapter error with `context`, the wire or socket error its cause.
    pub(crate) fn context(self, context: String) -> prysm_ports::Error {
        match self {
            Self::Wire(error) => prysm_ports::Error::adapter(context, error),
            Self::Socket(error) => prysm_ports::Error::adapter(context, error),
        }
    }

    /// A non-blocking write found the send buffer full; flushing completes it.
    pub(crate) fn would_block(&self) -> bool {
        matches!(self, Self::Socket(tungstenite::Error::Io(error))
            if error.kind() == ErrorKind::WouldBlock)
    }
}

/// Limits for the longest message an end reads.
pub(crate) fn config(longest: usize) -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(longest))
        .max_frame_size(Some(longest))
}

pub(crate) struct Link<S> {
    socket: WebSocket<S>,
}

impl<S: Read + Write> Link<S> {
    pub(crate) fn new(socket: WebSocket<S>) -> Self {
        Self { socket }
    }

    /// The stream under the WebSocket.
    pub(crate) fn socket_mut(&mut self) -> &mut S {
        self.socket.get_mut()
    }

    /// The next message; `None` when none arrived within the socket's read
    /// timeout (or, non-blocking, none is waiting).
    ///
    /// # Errors
    /// The peer closed the link, sent a text message longer than
    /// [`MAX_TEXT_BYTES`] (read whole first, within the [`config`] limit),
    /// or the socket failed.
    pub(crate) fn receive(&mut self) -> Result<Option<Message>, LinkError> {
        use tungstenite::Message as Ws;
        loop {
            match self.socket.read() {
                Ok(Ws::Text(text)) if text.len() > MAX_TEXT_BYTES => {
                    return Err(WireError::TextTooLarge(text.len()).into());
                }
                Ok(Ws::Text(text)) => return Ok(Some(Message::Text(text))),
                Ok(Ws::Binary(message)) => return Ok(Some(Message::Binary(message))),
                // Pongs are queued by the read and sent with the next write.
                Ok(Ws::Ping(_) | Ws::Pong(_) | Ws::Frame(_)) => {}
                Ok(Ws::Close(frame)) => {
                    let close = frame.and_then(|frame| Close::from_repr(frame.code.into()));
                    return Err(WireError::Closed(close).into());
                }
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    return Err(WireError::Closed(None).into());
                }
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Send one message. The WebSocket copies it into its write buffer (the
    /// client masks it there) and writes it out.
    ///
    /// # Errors
    /// The socket failed or timed out; [`LinkError::would_block`] on a full
    /// non-blocking socket, after which [`Self::flush`] completes the send.
    pub(crate) fn send(&mut self, message: Message) -> Result<(), LinkError> {
        let message = match message {
            Message::Text(text) => tungstenite::Message::Text(text),
            Message::Binary(bytes) => tungstenite::Message::Binary(bytes),
        };
        Ok(self.socket.send(message)?)
    }

    /// Write out what a send left buffered.
    ///
    /// # Errors
    /// The socket failed or timed out.
    pub(crate) fn flush(&mut self) -> Result<(), LinkError> {
        Ok(self.socket.flush()?)
    }

    /// Close the link, best effort, with `close`'s code if given: one write
    /// and its flush. Whether the close went out; when it did not (the
    /// peer is not reading, or the socket failed), it stays queued.
    pub(crate) fn close(&mut self, close: Option<Close>) -> bool {
        let frame = close.map(|close| CloseFrame {
            code: (close as u16).into(),
            reason: "".into(),
        });
        self.socket.close(frame).is_ok()
    }

    /// After a [`Self::close`] that went out: read and discard what the peer
    /// still sends, until it answers the close or the connection ends, for
    /// at most `within` plus one read timeout (reads wake at the socket's
    /// read timeout; once the close went out, a read writes nothing).
    /// Closing a socket with unread input resets the connection, which can
    /// lose the close before the peer reads it. After a close that did not
    /// go out, each read would retry it, blocking up to a write timeout.
    pub(crate) fn drain(&mut self, within: Duration) {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            match self.socket.read() {
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                    ) => {}
                Err(_) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use tungstenite::protocol::Role;

    /// A peer with nothing to say that takes every write, or none.
    struct Peer {
        reading: bool,
    }

    impl Read for Peer {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(ErrorKind::WouldBlock.into())
        }
    }

    impl Write for Peer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.reading {
                Ok(bytes.len())
            } else {
                Err(ErrorKind::TimedOut.into())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The receiver waits for an answer only after a close that went out:
    /// one to a peer that is not reading says it did not.
    #[test]
    fn a_close_says_whether_it_went_out() {
        for reading in [true, false] {
            let socket = WebSocket::from_raw_socket(Peer { reading }, Role::Server, None);
            assert_eq!(Link::new(socket).close(Some(Close::Stopped)), reading);
        }
    }
}
