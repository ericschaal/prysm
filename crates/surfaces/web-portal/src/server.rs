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
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout_at};

const CONNECTIONS: usize = 64;
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
        TcpListener::accept(self).await.map(|(socket, _)| socket)
    }
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{mpsc, oneshot};

    /// Scripted accepts: each a connection that lasts until its sender is
    /// used or dropped, or an error; nothing more once the script is done.
    struct Script(mpsc::UnboundedReceiver<io::Result<oneshot::Receiver<()>>>);

    impl Incoming for Script {
        type Socket = oneshot::Receiver<()>;

        async fn accept(&mut self) -> io::Result<Self::Socket> {
            match self.0.recv().await {
                Some(accepted) => accepted,
                None => std::future::pending().await,
            }
        }
    }

    /// Out of descriptors with a connection open, the loop stops accepting
    /// (the next connection waits in the script, and the loop does not spin:
    /// the paused clock only moves once everything is idle), then accepts
    /// again once that connection ends.
    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn an_exhausted_listener_accepts_again_once_a_connection_ends() {
        let (script, incoming) = mpsc::unbounded_channel();
        let (end_first, first) = oneshot::channel();
        let (_end_second, second) = oneshot::channel();
        script.send(Ok(first)).unwrap();
        script
            .send(Err(io::Error::from_raw_os_error(libc::EMFILE)))
            .unwrap();
        script.send(Ok(second)).unwrap();
        let served = Arc::new(AtomicUsize::new(0));
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn({
            let served = served.clone();
            async move {
                let mut incoming = Script(incoming);
                let mut connections = JoinSet::new();
                accept_until(
                    &mut incoming,
                    &mut connections,
                    async {
                        let _ = stopped.await;
                    },
                    |ends: oneshot::Receiver<()>| {
                        served.fetch_add(1, Ordering::SeqCst);
                        async move {
                            let _ = ends.await;
                        }
                    },
                )
                .await
            }
        });
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(served.load(Ordering::SeqCst), 1, "waits while exhausted");
        end_first.send(()).unwrap();
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(served.load(Ordering::SeqCst), 2, "accepts again");
        stop.send(()).unwrap();
        assert!(task.await.unwrap().is_ok());
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
        let mut incoming = Script(incoming);
        let failed = accept_until(
            &mut incoming,
            &mut JoinSet::new(),
            std::future::pending(),
            |_ends: oneshot::Receiver<()>| async {},
        )
        .await;
        assert!(failed.is_err());
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
