//! Listener and connection ownership. Every connection task, and every
//! capture stream upgrade it started, is joined (or, past the drain
//! deadline, aborted) before `serve` returns.
use crate::WebPortal;
use crate::routes::{self, BODY_DEADLINE, Context};
use crate::timed_io::TimedIo;
use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use prysm_ports::{Error, Result};
use prysm_runtime::{EngineHandle, Shutdown};
use socket2::{SockRef, TcpKeepalive};
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout_at};

const CONNECTIONS: usize = 64;
/// How long a connection is idle before the kernel probes its peer: under
/// the event streams' 15 s keepalive comment, so a quiet stream is probed.
const PROBE_AFTER: Duration = Duration::from_secs(10);
/// How long between unanswered probes.
const PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// Unanswered probes before the kernel ends the connection: an idle peer
/// that vanished is dropped 10 s + 4 x 5 s = 30 s after it last answered.
/// Linux reaches the same 30 s through `UNACKNOWLEDGED`, which replaces
/// the probe count there.
const PROBES: u32 = 4;
/// On Linux, how long sent data may stay unacknowledged before the kernel
/// ends the connection: the same 30 s for a peer that vanished mid-write.
#[cfg(target_os = "linux")]
const UNACKNOWLEDGED: Duration = Duration::from_secs(30);
/// How long open requests may take to finish once shutdown starts.
const DRAIN_DEADLINE: Duration = Duration::from_secs(5);

pub(crate) async fn serve(
    portal: WebPortal,
    engine: EngineHandle,
    shutdown: Shutdown,
) -> Result<()> {
    let listener = portal
        .listener
        .set_nonblocking(true)
        .and_then(|()| TcpListener::from_std(portal.listener))
        .map_err(|error| Error::adapter("cannot use the portal listener", error))?;
    let context = Arc::new(Context::new(
        engine,
        portal.assets,
        portal.access,
        portal.metrics,
        portal.capture,
        shutdown.clone(),
    ));
    let mut connections = JoinSet::new();
    let mut listener = listener;
    let result = accept_until(
        &mut listener,
        &mut connections,
        shutdown.requested(),
        |socket| connection(socket, context.clone()),
    )
    .await;
    // Refuse new connections at once; open ones finish their current request.
    drop(listener);
    let deadline = Instant::now() + DRAIN_DEADLINE;
    let drained = timeout_at(deadline, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        connections.shutdown().await;
    }
    // No connection is left to start another upgrade.
    let mut upgrades = context.take_upgrades();
    let handed_over = timeout_at(deadline, async {
        while upgrades.join_next().await.is_some() {}
    })
    .await;
    if handed_over.is_err() {
        upgrades.shutdown().await;
    }
    result
}

/// Where connections come from: the portal's listener, or a script in tests.
trait Incoming {
    type Socket;
    fn accept(&mut self) -> impl Future<Output = io::Result<Self::Socket>> + Send;
}

impl Incoming for TcpListener {
    type Socket = TcpStream;

    async fn accept(&mut self) -> io::Result<TcpStream> {
        let (socket, _) = TcpListener::accept(self).await?;
        // Each event is one write: without this, its last partial segment
        // can wait for the peer's delayed acknowledgement.
        if let Err(error) = socket.set_nodelay(true) {
            tracing::warn!(
                "a portal connection's events can wait for the peer's acknowledgement: {error}"
            );
        }
        if let Err(error) = detect_vanished_peer(&socket) {
            tracing::warn!(
                "a portal connection whose peer vanishes keeps its slot until a write fails: {error}"
            );
        }
        Ok(socket)
    }
}

/// Have the kernel end `socket` once its peer stops answering, so a reader
/// that vanished without closing (a laptop asleep, Wi-Fi gone) fails its
/// next read or write and frees its connection and stream slot. Keepalive
/// probes cover a connection with nothing in flight, on every platform; on
/// Linux the user timeout covers one with data unacknowledged (an event
/// stream's keepalive comment), which otherwise waits out TCP's
/// retransmission timeout (about 15 minutes there).
fn detect_vanished_peer(socket: &TcpStream) -> io::Result<()> {
    let socket = SockRef::from(socket);
    socket.set_tcp_keepalive(
        &TcpKeepalive::new()
            .with_time(PROBE_AFTER)
            .with_interval(PROBE_INTERVAL)
            .with_retries(PROBES),
    )?;
    #[cfg(target_os = "linux")]
    socket.set_tcp_user_timeout(Some(UNACKNOWLEDGED))?;
    Ok(())
}

/// Accept connections, each served by `serve` in `connections`, until
/// `stop` resolves or the listener fails. An accept that finds the process
/// out of descriptors or memory stops accepting until one of these
/// connections ends: only that wakes it, so while every open connection is
/// long-lived (an event stream, say) the portal accepts nothing, even after
/// descriptors were freed elsewhere in the process.
async fn accept_until<I, F>(
    incoming: &mut I,
    connections: &mut JoinSet<()>,
    stop: impl Future<Output = ()>,
    mut serve: impl FnMut(I::Socket) -> F,
) -> Result<()>
where
    I: Incoming,
    F: Future<Output = ()> + Send + 'static,
{
    tokio::pin!(stop);
    let mut exhausted = false;
    loop {
        tokio::select! {
            biased;
            () = &mut stop => return Ok(()),
            Some(_) = connections.join_next(), if !connections.is_empty() => exhausted = false,
            accepted = incoming.accept(), if !exhausted && connections.len() < CONNECTIONS => {
                match accepted {
                    Ok(socket) => {
                        connections.spawn(serve(socket));
                    }
                    Err(error) => match refused(&error, connections.len()) {
                        Refused::Connection => {}
                        Refused::Exhausted => {
                            tracing::warn!(
                                "the portal accepts again once one of its connections ends: {error}"
                            );
                            exhausted = true;
                        }
                        Refused::Listener => {
                            return Err(Error::adapter("portal listener failed", error));
                        }
                    },
                }
            }
        }
    }
}

/// What a failed `accept` means for the portal.
#[derive(Debug, PartialEq, Eq)]
enum Refused {
    /// One pending connection failed; the next is accepted.
    Connection,
    /// The process is out of descriptors or memory: accepting resumes once
    /// a connection ends and frees what it held.
    Exhausted,
    /// The listener itself failed, or it is exhausted with no connection
    /// open whose end would free anything: the portal stops.
    Listener,
}

/// What `error` from `accept` means while `open` connections are served.
fn refused(error: &io::Error, open: usize) -> Refused {
    if matches!(
        error.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
    ) {
        return Refused::Connection;
    }
    #[cfg(unix)]
    let exhausted = matches!(
        error.raw_os_error(),
        Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM)
    );
    #[cfg(not(unix))]
    let exhausted = error.kind() == io::ErrorKind::OutOfMemory;
    if exhausted && open > 0 {
        Refused::Exhausted
    } else {
        Refused::Listener
    }
}

async fn connection(socket: TcpStream, context: Arc<Context>) {
    let shutdown = context.shutdown.clone();
    let service = service_fn(move |request| {
        let context = context.clone();
        async move { Ok::<_, Infallible>(routes::handle(context, request).await) }
    });
    let connection = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(BODY_DEADLINE)
        .serve_connection(TokioIo::new(TimedIo::new(socket)), service)
        .with_upgrades();
    tokio::pin!(connection);
    tokio::select! {
        _ = connection.as_mut() => {}
        () = shutdown.requested() => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::{mpsc, oneshot};

    /// Scripted accepts: each a connection that lasts until its sender is
    /// used or dropped, or an error; nothing more once the script is done.
    /// Each accept called is told on `calls`; none may come while `holding`
    /// (the test's guard against accepting while exhausted).
    struct Script {
        accepts: mpsc::UnboundedReceiver<io::Result<oneshot::Receiver<()>>>,
        calls: mpsc::UnboundedSender<()>,
        holding: Arc<AtomicBool>,
    }

    impl Incoming for Script {
        type Socket = oneshot::Receiver<()>;

        async fn accept(&mut self) -> io::Result<Self::Socket> {
            assert!(
                !self.holding.load(Ordering::SeqCst),
                "accepted while exhausted"
            );
            let _ = self.calls.send(());
            match self.accepts.recv().await {
                Some(accepted) => accepted,
                None => std::future::pending().await,
            }
        }
    }

    /// Out of descriptors with a connection open, the loop stops accepting
    /// until that connection ends, then accepts again. A loop that accepted
    /// meanwhile fails the script's guard; one that spun would never let the
    /// test go on, which the time limit turns into a failure.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_exhausted_listener_accepts_again_once_a_connection_ends() {
        prysm_testkit::within_async(Duration::from_secs(10), async {
            let (script, accepts) = mpsc::unbounded_channel();
            let (calls_tx, mut calls) = mpsc::unbounded_channel();
            let (served_tx, mut served) = mpsc::unbounded_channel();
            let holding = Arc::new(AtomicBool::new(false));
            let (end_first, first) = oneshot::channel();
            let (_end_second, second) = oneshot::channel();
            script.send(Ok(first)).unwrap();
            script
                .send(Err(io::Error::from_raw_os_error(libc::EMFILE)))
                .unwrap();
            let (stop, stopped) = oneshot::channel::<()>();
            let mut incoming = Script {
                accepts,
                calls: calls_tx,
                holding: holding.clone(),
            };
            let task = tokio::spawn(async move {
                let mut connections = JoinSet::new();
                accept_until(
                    &mut incoming,
                    &mut connections,
                    async {
                        let _ = stopped.await;
                    },
                    |ends: oneshot::Receiver<()>| {
                        let _ = served_tx.send(());
                        async move {
                            let _ = ends.await;
                        }
                    },
                )
                .await
            });
            // Accept 1 serves the first connection; accept 2 meets EMFILE,
            // which the loop handles in the poll that got it.
            calls.recv().await.unwrap();
            served.recv().await.unwrap();
            calls.recv().await.unwrap();
            // Exhausted: another connection waits, and the loop runs once
            // more without accepting it.
            holding.store(true, Ordering::SeqCst);
            script.send(Ok(second)).unwrap();
            tokio::task::yield_now().await;
            holding.store(false, Ordering::SeqCst);
            end_first.send(()).unwrap();
            calls.recv().await.unwrap();
            served.recv().await.unwrap();
            stop.send(()).unwrap();
            assert!(task.await.unwrap().is_ok());
        })
        .await;
    }

    /// Out of descriptors with no connection open, nothing would wake it:
    /// the listener fails.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_exhausted_listener_without_connections_fails() {
        let (script, incoming) = mpsc::unbounded_channel();
        script
            .send(Err(io::Error::from_raw_os_error(libc::EMFILE)))
            .unwrap();
        let (calls, _) = mpsc::unbounded_channel();
        let mut incoming = Script {
            accepts: incoming,
            calls,
            holding: Arc::default(),
        };
        let failed = accept_until(
            &mut incoming,
            &mut JoinSet::new(),
            std::future::pending(),
            |_ends: oneshot::Receiver<()>| async {},
        )
        .await;
        assert!(failed.is_err());
    }

    /// An accepted connection carries the kernel options that end it once
    /// its peer stops answering.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_accepted_connection_detects_a_vanished_peer() {
        let mut listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (_client, accepted) = tokio::join!(TcpStream::connect(address), async {
            Incoming::accept(&mut listener).await
        });
        let accepted = accepted.unwrap();
        let socket = SockRef::from(&accepted);
        assert!(socket.keepalive().unwrap());
        assert_eq!(socket.tcp_keepalive_time().unwrap(), PROBE_AFTER);
        assert_eq!(socket.tcp_keepalive_interval().unwrap(), PROBE_INTERVAL);
        assert_eq!(socket.tcp_keepalive_retries().unwrap(), PROBES);
        #[cfg(target_os = "linux")]
        assert_eq!(socket.tcp_user_timeout().unwrap(), Some(UNACKNOWLEDGED));
    }

    /// An accepted connection sends each write without waiting to coalesce
    /// it with the next (Nagle's algorithm off).
    #[tokio::test]
    async fn an_accepted_connection_sends_without_delay() {
        let mut listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (_client, accepted) = tokio::join!(TcpStream::connect(address), async {
            Incoming::accept(&mut listener).await
        });
        assert!(accepted.unwrap().nodelay().unwrap());
    }

    /// Every kind of `accept` failure, with and without open connections.
    #[cfg(unix)]
    #[test]
    fn an_exhausted_listener_waits_for_a_connection_to_end() {
        let os = io::Error::from_raw_os_error;
        let kind = io::Error::from;
        #[rustfmt::skip]
        let cases = [
            (kind(io::ErrorKind::ConnectionAborted), 0, Refused::Connection),
            (kind(io::ErrorKind::ConnectionReset), 3, Refused::Connection),
            (kind(io::ErrorKind::Interrupted), 0, Refused::Connection),
            (kind(io::ErrorKind::WouldBlock), 0, Refused::Connection),
            (os(libc::EMFILE), 1, Refused::Exhausted),
            (os(libc::ENFILE), 64, Refused::Exhausted),
            (os(libc::ENOBUFS), 2, Refused::Exhausted),
            (os(libc::ENOMEM), 2, Refused::Exhausted),
            (os(libc::EMFILE), 0, Refused::Listener),
            (os(libc::ENOMEM), 0, Refused::Listener),
            (os(libc::EBADF), 5, Refused::Listener),
            (os(libc::EINVAL), 0, Refused::Listener),
        ];
        for (error, open, expected) in cases {
            assert_eq!(refused(&error, open), expected, "{error} with {open} open");
        }
    }
}
