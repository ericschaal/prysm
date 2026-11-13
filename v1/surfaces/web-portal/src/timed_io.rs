//! Bound stalled socket writes without imposing a lifetime on idle observations,
//! and close without resetting a reply the client has yet to read.
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll, ready},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    time::{Sleep, sleep},
};

/// How long a closing connection keeps discarding what the client still sends.
const LINGER: Duration = Duration::from_secs(2);

pub(super) struct TimedIo {
    socket: TcpStream,
    stalled: Option<Pin<Box<Sleep>>>,
    /// Set once the write side is shut down: the end of the lingering close.
    lingering: Option<Pin<Box<Sleep>>>,
}
impl TimedIo {
    /// The socket, for a connection upgraded away from HTTP.
    pub fn into_socket(self) -> TcpStream {
        self.socket
    }

    pub fn new(socket: TcpStream) -> Self {
        Self {
            socket,
            stalled: None,
            lingering: None,
        }
    }
    fn write_result<T>(
        &mut self,
        result: Poll<io::Result<T>>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<T>> {
        if result.is_ready() {
            self.stalled = None;
            return result;
        }
        let deadline = self
            .stalled
            .get_or_insert_with(|| Box::pin(sleep(Duration::from_secs(5))));
        if deadline.as_mut().poll(cx).is_ready() {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "portal socket write stalled",
            )))
        } else {
            Poll::Pending
        }
    }
}
impl AsyncRead for TimedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_read(cx, buf)
    }
}
impl AsyncWrite for TimedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.socket).poll_write(cx, buf);
        self.write_result(result, cx)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.socket).poll_flush(cx);
        self.write_result(result, cx)
    }
    /// A lingering close. Hyper answers some requests (a refused origin, a
    /// `Connection: close` reply) before reading their whole body and then
    /// drops the socket. Closing a socket with unread bytes, or receiving bytes
    /// after closing, makes the kernel send a reset, and the client may lose the
    /// reply it has not read yet. So: send FIN, then discard input until the
    /// client closes too or [`LINGER`] passes.
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if this.lingering.is_none() {
            ready!(Pin::new(&mut this.socket).poll_shutdown(cx))?;
            this.lingering = Some(Box::pin(sleep(LINGER)));
        }
        let mut discard = [0; 1024];
        loop {
            if this
                .lingering
                .as_mut()
                .is_none_or(|deadline| deadline.as_mut().poll(cx).is_ready())
            {
                return Poll::Ready(Ok(()));
            }
            let mut buf = ReadBuf::new(&mut discard);
            match ready!(Pin::new(&mut this.socket).poll_read(cx, &mut buf)) {
                Ok(()) if !buf.filled().is_empty() => {}
                // End of input or a broken connection: nothing left to protect.
                _ => return Poll::Ready(Ok(())),
            }
        }
    }
}
