//! Ending an adapter's blocking network calls from another thread, the
//! moment the one who started them stops.
use std::fmt;
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

/// Ends the blocking calls registered with it the moment [`Hangup::cut`] is
/// called: it shuts their sockets down, which wakes a blocked connect, read
/// or write at once, and wakes a wait for a name or for the cut itself;
/// every later connect or wait ends at once. The runtime cuts a session's
/// when the session is asked to stop, and a host cuts its own when it is, so
/// a stop never waits for a device or a receiver. Cheap to clone; clones
/// share the state. A default one is cut only by its holders.
#[derive(Debug, Clone, Default)]
pub struct Hangup(Arc<Mutex<Lines>>);

/// Why a call ended when its [`Hangup`] was cut: its caller is stopping,
/// whatever the socket said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("ended by a stop")]
pub struct Stopping;

/// Why a call that a [`Hangup`] can end failed: the cut ended it, whatever
/// the socket or the lookup said, or it failed on its own.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    /// [`Error::cause`](crate::Error::cause) finds the [`Stopping`].
    #[error("call ended")]
    Stopped(#[from] Stopping),
    #[error(transparent)]
    Failed(#[from] io::Error),
}

/// What a [`Hangup`] cuts, by registration number.
#[derive(Debug, Default)]
struct Lines {
    cut: bool,
    next: u64,
    lines: Vec<(u64, Line)>,
    /// Blocking waits begun (a connect's, or a caller's it counts with
    /// [`Hangup::waiting`]), so a test cuts only a call that waits.
    #[cfg(feature = "testkit")]
    waits: usize,
}

/// One registered call.
enum Line {
    /// Shut down by the cut.
    Socket(TcpStream),
    /// Called by the cut, to wake a wait that is not on a socket.
    Wake(Box<dyn FnOnce() + Send>),
}

impl fmt::Debug for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(socket) => f.debug_tuple("Socket").field(socket).finish(),
            Self::Wake(_) => f.write_str("Wake"),
        }
    }
}

impl Hangup {
    /// End every registered call, and every later one at its start.
    pub fn cut(&self) {
        let mut lines = self.lines();
        lines.cut = true;
        for (_, line) in lines.lines.drain(..) {
            match line {
                Line::Socket(socket) => {
                    let _ = socket.shutdown(std::net::Shutdown::Both);
                }
                Line::Wake(wake) => wake(),
            }
        }
    }

    /// Whether [`Self::cut`] was called.
    #[must_use]
    pub fn is_cut(&self) -> bool {
        self.lines().cut
    }

    /// Connect a TCP socket to `address` by `deadline`, cut with the others
    /// until the [`Registration`] drops. The socket is connecting before a
    /// cut can reach it (both under the lock), so a cut ends the wait at
    /// once, and the shutdown then ends every read and write on it.
    ///
    /// # Errors
    /// [`CallError::Stopped`] once cut, before or during the connect; else
    /// the connect failed or did not end by `deadline`
    /// ([`io::ErrorKind::TimedOut`]).
    pub fn connect(
        &self,
        address: SocketAddr,
        deadline: Instant,
    ) -> Result<(TcpStream, Registration), CallError> {
        self.try_connect(address, deadline)
            .map_err(|error| self.ended(error))
    }

    /// The cut, once it came, else `error`.
    pub(crate) fn ended(&self, error: io::Error) -> CallError {
        if self.is_cut() {
            Stopping.into()
        } else {
            error.into()
        }
    }

    fn try_connect(
        &self,
        address: SocketAddr,
        deadline: Instant,
    ) -> io::Result<(TcpStream, Registration)> {
        let socket = socket2::Socket::new(
            socket2::Domain::for_address(address),
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )?;
        socket.set_nonblocking(true)?;
        let registration = {
            let mut lines = self.lines();
            // Reported as the cut by `connect`.
            if lines.cut {
                return Err(io::ErrorKind::NotConnected.into());
            }
            match socket.connect(&address.into()) {
                Ok(()) => {}
                Err(error)
                    if error.raw_os_error() == Some(nix::libc::EINPROGRESS)
                        || error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            lines.register(self, Line::Socket(socket.try_clone()?.into()))
        };
        self.connected(&socket, deadline)?;
        socket.set_nonblocking(false)?;
        Ok((socket.into(), registration))
    }

    /// Wait until the non-blocking `socket`'s connect ends, by `deadline`.
    fn connected(&self, socket: &socket2::Socket, deadline: Instant) -> io::Result<()> {
        use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
        use std::os::fd::AsFd;
        self.waiting();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            // Rounded up to poll's milliseconds, so the last one is waited, not spun.
            let timeout =
                PollTimeout::try_from(left + Duration::from_millis(1)).unwrap_or(PollTimeout::MAX);
            let mut fds = [PollFd::new(socket.as_fd(), PollFlags::POLLOUT)];
            match poll(&mut fds, timeout) {
                Ok(0) | Err(nix::errno::Errno::EINTR) => {}
                Ok(_) => break,
                Err(error) => return Err(error.into()),
            }
        }
        if let Some(error) = socket.take_error()? {
            return Err(error);
        }
        // A connect that a shutdown ended has no error, and no peer.
        socket.peer_addr().map(drop)
    }

    /// Call `wake` when the cut comes while the [`Registration`] lives;
    /// `None` once cut, without calling it.
    pub(crate) fn on_cut(&self, wake: impl FnOnce() + Send + 'static) -> Option<Registration> {
        let mut lines = self.lines();
        (!lines.cut).then(|| lines.register(self, Line::Wake(Box::new(wake))))
    }

    /// Block until the cut, at most `timeout`; whether it came.
    #[must_use]
    pub fn wait(&self, timeout: Duration) -> bool {
        let (woken, wake) = mpsc::channel();
        let Some(_registration) = self.on_cut(move || {
            let _ = woken.send(());
        }) else {
            return true;
        };
        self.waiting();
        wake.recv_timeout(timeout).is_ok()
    }

    /// A blocking wait begins: counted with the `testkit` feature, nothing
    /// otherwise.
    pub fn waiting(&self) {
        #[cfg(feature = "testkit")]
        {
            self.lines().waits += 1;
        }
    }

    /// The blocking waits begun so far.
    #[cfg(feature = "testkit")]
    #[must_use]
    pub fn waits(&self) -> usize {
        self.lines().waits
    }

    fn lines(&self) -> MutexGuard<'_, Lines> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Lines {
    fn register(&mut self, hangup: &Hangup, line: Line) -> Registration {
        let id = self.next;
        self.next += 1;
        self.lines.push((id, line));
        Registration {
            hangup: hangup.clone(),
            id,
        }
    }
}

/// A call a [`Hangup`] cuts; dropping it ends the [`Hangup`]'s hold on it
/// (for a socket, its handle, so the connection can close).
#[derive(Debug)]
pub struct Registration {
    hangup: Hangup,
    id: u64,
}

impl Registration {
    /// The [`Hangup`] that cuts this call.
    #[must_use]
    pub fn hangup(&self) -> &Hangup {
        &self.hangup
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.hangup.lines().lines.retain(|(id, _)| *id != self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// A wait ends at the cut, and every wait after it at once.
    #[test]
    fn a_cut_ends_a_wait_and_every_later_one() {
        let hangup = Hangup::default();
        let waiting = {
            let hangup = hangup.clone();
            thread::spawn(move || hangup.wait(Duration::from_secs(60)))
        };
        // Cut before the wait began, or while it waits: either way it ends.
        hangup.cut();
        assert!(waiting.join().unwrap());
        assert!(hangup.wait(Duration::from_secs(60)));
    }

    /// Without a cut, a wait lasts its timeout.
    #[test]
    fn a_wait_without_a_cut_times_out() {
        assert!(!Hangup::default().wait(Duration::from_millis(10)));
    }

    /// A connect after the cut fails with [`Stopping`] without connecting.
    #[test]
    fn a_connect_after_the_cut_fails_at_once() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let hangup = Hangup::default();
        hangup.cut();
        let error = hangup
            .connect(
                listener.local_addr().unwrap(),
                Instant::now() + Duration::from_secs(60),
            )
            .unwrap_err();
        assert!(matches!(error, CallError::Stopped(Stopping)), "{error}");
    }

    /// Registrations end with their calls: a cut after them wakes nothing.
    #[test]
    fn a_dropped_registration_is_not_cut() {
        let hangup = Hangup::default();
        let woken = Arc::new(Mutex::new(false));
        let registration = {
            let woken = woken.clone();
            hangup.on_cut(move || *woken.lock().unwrap() = true)
        };
        drop(registration);
        hangup.cut();
        assert!(!*woken.lock().unwrap());
    }
}
