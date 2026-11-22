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
    // An accept that found the process out of descriptors or memory: the
    // next waits until a connection ends and frees what it held.
    let mut exhausted = false;
    let result = loop {
        tokio::select! {
            biased;
            () = shutdown.requested() => break Ok(()),
            Some(_) = connections.join_next(), if !connections.is_empty() => exhausted = false,
            accepted = listener.accept(), if !exhausted && connections.len() < CONNECTIONS => {
                match accepted {
                    Ok((socket, _)) => {
                        connections.spawn(connection(socket, context.clone()));
                    }
                    Err(error) => match refused(&error, connections.len()) {
                        Refused::Connection => {}
                        Refused::Exhausted => {
                            tracing::warn!(
                                "the portal accepts again once a connection ends: {error}"
                            );
                            exhausted = true;
                        }
                        Refused::Listener => {
                            break Err(Error::adapter("portal listener failed", error));
                        }
                    },
                }
            }
        }
    };
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
