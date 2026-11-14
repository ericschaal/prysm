//! One WebSocket, either end: text messages (JSON commands and refusals)
//! and binary messages (frames). The ends never see WebSocket types.
use crate::wire::WireError;
use bytes::Bytes;
use std::io::{ErrorKind, Read, Write};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

/// One message from the peer.
pub(crate) enum Inbound {
    Text(String),
    Binary(Bytes),
}

/// One message to the peer.
pub(crate) enum Outbound {
    Text(String),
    Binary(Bytes),
}

/// Why a link ended.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LinkError {
    /// The peer closed it, or sent what this end does not take.
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

    /// The next message; `None` when none arrived within the socket's read
    /// timeout (or, non-blocking, none is waiting).
    ///
    /// # Errors
    /// The peer closed the link, or the socket failed.
    pub(crate) fn receive(&mut self) -> Result<Option<Inbound>, LinkError> {
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    return Ok(Some(Inbound::Text(text.as_str().to_owned())));
                }
                Ok(Message::Binary(message)) => return Ok(Some(Inbound::Binary(message))),
                // Pongs are queued by the read and sent with the next write.
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Close(_))
                | Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    return Err(WireError::Closed.into());
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
    pub(crate) fn send(&mut self, message: Outbound) -> Result<(), LinkError> {
        let message = match message {
            Outbound::Text(text) => Message::text(text),
            Outbound::Binary(bytes) => Message::Binary(bytes),
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

    /// Close the link, best effort.
    pub(crate) fn close(&mut self) {
        let _ = self.socket.close(None);
        let _ = self.socket.flush();
    }
}
